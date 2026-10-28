import { backoffDelayMs } from "../lib/backoff.ts";
import type { AuthProvider } from "./auth.ts";
import { isTransient } from "./failure.ts";
import type { ApiFailure, FailureContext } from "./failure.ts";

/** Who is asking, most urgent first. */
export type Priority = "action" | "view" | "poll";

const RANK: Record<Priority, number> = { action: 0, view: 1, poll: 2 };

/** Requests wait this long for the device unless the operation says otherwise. */
export const TIMEOUT_MS = 8_000;
/** Requests with a body wait longer, as the server reads those for longer. */
export const TIMEOUT_WITH_BODY_MS = 10_000;
/** The most times an idempotent request is tried. */
export const MAX_ATTEMPTS = 4;
/** The longest the device can ask a retry to wait. */
const MAX_RETRY_AFTER_MS = 30_000;

export interface RequestSpec {
  method: "GET" | "POST" | "PUT" | "DELETE";
  /** The path on the device, such as `/api/v1/system`. */
  path: string;
  body?: unknown;
  /** Whether the request can be repeated without a further effect. */
  idempotent: boolean;
  priority: Priority;
  timeoutMs?: number;
  /** Polls of one resource coalesce into a single request. */
  resource?: string;
  /** Leaving the page this names aborts its view loads. */
  group?: string;
  /** Whether to try again after a transient failure; by default, whether the request is idempotent and not a poll. */
  retry?: boolean;
}

export interface Reply {
  status: number;
  /** The parsed JSON body, or `undefined` if the response has none. */
  body: unknown;
}

export type Outcome =
  | { ok: true; reply: Reply; context: FailureContext }
  | { ok: false; failure: ApiFailure; context: FailureContext };

export interface Timers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface TransportDeps {
  fetch: (url: string, init: RequestInit) => Promise<Response>;
  timers: Timers;
  /** A value in [0, 1). */
  random: () => number;
  /** Milliseconds since the epoch. */
  now: () => number;
  auth: AuthProvider;
  /** Called when the queue pauses for a login. */
  onLoginRequired: () => void;
  /**
   * Called after every attempt the page didn't cancel, with whether the device
   * answered and wasn't too busy to take the request.
   */
  onAttempt?: (reachable: boolean) => void;
}

interface Job {
  spec: RequestSpec;
  seq: number;
  attempts: number;
  aborted: boolean;
  current: AbortController | null;
  wake: (() => void) | null;
  promise: Promise<Outcome>;
  resolve: (outcome: Outcome) => void;
}

type Attempt = { ok: true; reply: Reply } | { ok: false; failure: ApiFailure };

/**
 * Sends every request of the page, one at a time: the device has few
 * connections to share. Urgent requests go first, polls of one resource
 * coalesce, and only requests that can be repeated are retried.
 */
export class Transport {
  readonly #deps: TransportDeps;
  readonly #queue: Job[] = [];
  #running: Job | null = null;
  #paused = false;
  #seq = 0;

  constructor(deps: TransportDeps) {
    this.#deps = deps;
  }

  /** Requests waiting or running. */
  get pending(): number {
    return this.#queue.length + (this.#running === null ? 0 : 1);
  }

  /** Whether the queue is waiting for a login. */
  get paused(): boolean {
    return this.#paused;
  }

  /** Queues a request. The promise settles with its outcome and never rejects. */
  send(spec: RequestSpec): Promise<Outcome> {
    if (spec.priority === "poll" && spec.resource !== undefined) {
      const same = [...this.#queue, ...(this.#running === null ? [] : [this.#running])].find(
        (job) => job.spec.priority === "poll" && job.spec.resource === spec.resource,
      );
      if (same !== undefined) return same.promise;
    }
    let resolve!: (outcome: Outcome) => void;
    const promise = new Promise<Outcome>((done) => {
      resolve = done;
    });
    this.#queue.push({
      spec,
      seq: this.#seq++,
      attempts: 0,
      aborted: false,
      current: null,
      wake: null,
      promise,
      resolve,
    });
    this.#pump();
    return promise;
  }

  /** Aborts the queued and running view loads of `group`. */
  abortGroup(group: string): void {
    const doomed = (job: Job): boolean => job.spec.priority === "view" && job.spec.group === group;
    for (const job of this.#queue.filter(doomed)) {
      this.#queue.splice(this.#queue.indexOf(job), 1);
      job.resolve(this.#fail(job, { kind: "aborted" }));
    }
    const running = this.#running;
    if (running !== null && doomed(running)) {
      running.aborted = true;
      running.current?.abort();
      running.wake?.();
    }
  }

  /** Continues after a login. */
  resume(): void {
    this.#paused = false;
    this.#pump();
  }

  #context(job: Job): FailureContext {
    return {
      method: job.spec.method,
      path: job.spec.path,
      attempts: job.attempts,
      at: this.#deps.now(),
    };
  }

  #fail(job: Job, failure: ApiFailure): Outcome {
    return { ok: false, failure, context: this.#context(job) };
  }

  #pump(): void {
    if (this.#running !== null || this.#paused) return;
    let best: Job | undefined;
    for (const job of this.#queue) {
      if (
        best === undefined ||
        RANK[job.spec.priority] < RANK[best.spec.priority] ||
        (job.spec.priority === best.spec.priority && job.seq < best.seq)
      ) {
        best = job;
      }
    }
    if (best === undefined) return;
    this.#queue.splice(this.#queue.indexOf(best), 1);
    this.#running = best;
    const job = best;
    void this.#run(job).then((outcome) => {
      this.#running = null;
      if (outcome !== "requeued") job.resolve(outcome);
      this.#pump();
    });
  }

  #canRetry(spec: RequestSpec): boolean {
    return spec.retry ?? (spec.idempotent && spec.priority !== "poll");
  }

  async #run(job: Job): Promise<Outcome | "requeued"> {
    for (;;) {
      if (job.aborted) return this.#fail(job, { kind: "aborted" });
      job.attempts += 1;
      const attempt = await this.#attempt(job);
      if (attempt.ok || attempt.failure.kind !== "aborted") {
        const failed =
          !attempt.ok && ["network", "timeout", "busy"].includes(attempt.failure.kind);
        this.#deps.onAttempt?.(!failed);
      }
      if (attempt.ok) {
        return { ok: true, reply: attempt.reply, context: this.#context(job) };
      }
      const { failure } = attempt;

      if (failure.kind === "unauthorized") {
        const decision = await this.#deps.auth.onUnauthorized();
        if (decision === "retry" && job.spec.idempotent && job.attempts < MAX_ATTEMPTS) continue;
        if (decision === "login-required") {
          this.#paused = true;
          this.#deps.onLoginRequired();
          if (job.spec.idempotent) {
            // Waits at the front of the queue until the person has signed in.
            job.seq = -1;
            job.attempts = 0;
            this.#queue.push(job);
            return "requeued";
          }
        }
        return this.#fail(job, failure);
      }

      if (this.#canRetry(job.spec) && isTransient(failure) && job.attempts < MAX_ATTEMPTS) {
        await this.#wait(job, this.#retryDelayMs(job, failure));
        continue;
      }

      const unknownOutcome =
        !job.spec.idempotent && (failure.kind === "network" || failure.kind === "timeout");
      return this.#fail(job, unknownOutcome ? { kind: "unknown-outcome", cause: failure } : failure);
    }
  }

  #retryDelayMs(job: Job, failure: ApiFailure): number {
    const backoff = backoffDelayMs(job.attempts - 1, this.#deps.random);
    if (failure.kind !== "busy" || failure.retryAfterS === undefined) return backoff;
    return Math.min(MAX_RETRY_AFTER_MS, Math.max(backoff, failure.retryAfterS * 1_000));
  }

  #wait(job: Job, ms: number): Promise<void> {
    const { timers } = this.#deps;
    return new Promise((done) => {
      const handle = timers.setTimeout(() => {
        job.wake = null;
        done();
      }, ms);
      job.wake = () => {
        timers.clearTimeout(handle);
        job.wake = null;
        done();
      };
    });
  }

  async #attempt(job: Job): Promise<Attempt> {
    const { spec } = job;
    const { timers } = this.#deps;
    const timeoutMs = spec.timeoutMs ?? (spec.body === undefined ? TIMEOUT_MS : TIMEOUT_WITH_BODY_MS);
    const controller = new AbortController();
    job.current = controller;
    let timedOut = false;
    const timer = timers.setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, timeoutMs);

    const headers: Record<string, string> = { Accept: "application/json" };
    const init: RequestInit = {
      method: spec.method,
      headers,
      signal: controller.signal,
      credentials: "same-origin",
      cache: "no-store",
      redirect: "error",
      referrerPolicy: "no-referrer",
    };
    if (spec.body !== undefined) {
      headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(spec.body);
    }

    try {
      const response = await this.#deps.fetch(spec.path, this.#deps.auth.decorate(init));
      const text = await response.text();
      return classify(response, text);
    } catch {
      if (job.aborted) return { ok: false, failure: { kind: "aborted" } };
      if (timedOut) return { ok: false, failure: { kind: "timeout", afterMs: timeoutMs } };
      return { ok: false, failure: { kind: "network" } };
    } finally {
      timers.clearTimeout(timer);
      job.current = null;
    }
  }
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

/** Reads the answer of the device as a reply or the failure it amounts to. */
export function classify(response: Response, text: string): Attempt {
  const { status } = response;
  let body: unknown;
  if (text !== "") {
    const type = response.headers.get("Content-Type") ?? "";
    if (!/^application\/json\b/i.test(type)) return { ok: false, failure: { kind: "decode", status } };
    try {
      body = JSON.parse(text);
    } catch {
      return { ok: false, failure: { kind: "decode", status } };
    }
  }

  if (status >= 200 && status < 300) return { ok: true, reply: { status, body } };
  if (status === 401) return { ok: false, failure: { kind: "unauthorized" } };
  if (status === 403) return { ok: false, failure: { kind: "forbidden" } };
  if (status === 409 || status === 423) {
    const fields = asRecord(body);
    const failure: ApiFailure = { kind: "locked" };
    if (typeof fields?.["holder"] === "string") failure.holder = fields["holder"];
    if (typeof fields?.["since"] === "string") failure.since = fields["since"];
    return { ok: false, failure };
  }
  if (status === 429 || status === 503) {
    const failure: ApiFailure = { kind: "busy" };
    const retryAfter = Number(response.headers.get("Retry-After"));
    if (Number.isFinite(retryAfter) && retryAfter > 0) failure.retryAfterS = retryAfter;
    return { ok: false, failure };
  }
  const code = asRecord(body)?.["error"];
  if (typeof code === "string") return { ok: false, failure: { kind: "api", status, code } };
  return { ok: false, failure: { kind: "decode", status } };
}
