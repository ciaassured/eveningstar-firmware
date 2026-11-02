import type { ReadonlySignal } from "@preact/signals";
import { computed, signal } from "@preact/signals";
import type { ApiClient } from "../api/client.ts";
import type { ApiFailure } from "../api/failure.ts";
import type { DownloadCall, DownloadHead, Timers } from "../api/transport.ts";
import { backoffDelayMs } from "../lib/backoff.ts";
import { unverifiedFilename } from "../storage/filenames.ts";
import type { DownloadPlan, DownloadTarget, PlanResult } from "../storage/plan.ts";

/** Where a download has got to. */
export type EntryState =
  | { kind: "queued" }
  /** Another client holds the device's one download; asks again at `until`. */
  | { kind: "waiting"; since: number; until: number }
  /** `total` is `undefined` while the download is being planned. */
  | { kind: "downloading"; received: number; total: number | undefined; retrying: boolean }
  | { kind: "verifying" }
  /** `checked` when the bytes matched the digest; `unchecked` when the page couldn't check; `mismatch` when saved anyway. */
  | { kind: "done"; verified: "checked" | "unchecked" | "mismatch" }
  /** `received` is how many bytes had arrived. */
  | { kind: "failed"; reason: string; resumable: boolean; received: number; mismatch?: { expected: string; actual: string } }
  | { kind: "cancelled" };

export interface DownloadEntry {
  readonly id: number;
  readonly target: DownloadTarget;
  /** When it was queued, or queued again by a retry. */
  readonly queuedAt: number;
  /** The plan the download runs to, once it has started. */
  readonly plan?: DownloadPlan;
  readonly state: EntryState;
  /** Why the download started again, when it did. */
  readonly note?: string;
}

/**
 * `retry` queues a failed or cancelled entry again: a resumable failure goes
 * on from the bytes kept, anything else starts over.
 */
export type EntryAction = "cancel" | "retry" | "dismiss" | "save-anyway";

/** What the download service needs of the page and the browser. */
export interface DownloadsDeps {
  api: Pick<ApiClient, "download">;
  /** Plans a download against what the device reports now. */
  prepare(target: DownloadTarget): Promise<PlanResult>;
  timers: Timers;
  now: () => number;
  random: () => number;
  /** Hands the finished bytes to the browser to save under `filename`. */
  save(bytes: Uint8Array, filename: string): void;
  /** The SHA-256 of the bytes in lowercase hex; `undefined` outside a secure context, where the page can't hash. */
  sha256: ((bytes: Uint8Array) => Promise<string>) | undefined;
  /** Asks the browser to confirm before the tab is closed or reloaded, while `on`. */
  guardUnload(on: boolean): void;
}

/** The app-wide download queue. */
export interface Downloads {
  /** Every entry, in queue order: oldest first. */
  readonly entries: ReadonlySignal<readonly DownloadEntry[]>;
  /**
   * Whether closing the tab would lose something: an entry queued, waiting,
   * downloading or verifying, or failed with its bytes kept for a resume.
   */
  readonly pending: ReadonlySignal<boolean>;
  /** Adds a download at the end of the queue. Returns its id. */
  add(target: DownloadTarget): number;
  act(id: number, action: EntryAction): void;
}

/** The waits before each resume of a broken download, long enough to outlast a restart of the device. */
export const RESUME_DELAYS_MS: readonly number[] = [1_000, 3_000, 10_000, 30_000];
/** How many times a broken download is resumed before it waits for the person. */
export const MAX_RESUMES = RESUME_DELAYS_MS.length;
/** How many times a download starts again because the region changed before it gives up. */
export const MAX_RESTARTS = 3;
/** The wait when the device is busy and doesn't say for how long. */
export const BUSY_WAIT_S = 5;
/** The longest a busy device can ask a download to wait between asks. */
export const MAX_BUSY_WAIT_S = 60;
/** Progress is published at most this often. */
export const PROGRESS_MS = 100;

/** The bytes and state of a running or resumable entry, kept out of the signals. */
interface Runtime {
  buffer?: Uint8Array;
  received: number;
  etag?: string;
  resumes: number;
  restarts: number;
  call?: DownloadCall;
  wake?: () => void;
  cancelled: boolean;
}

const TERMINAL: readonly EntryState["kind"][] = ["done", "failed", "cancelled"];

function brokenReason(failure: ApiFailure): string {
  switch (failure.kind) {
    case "timeout":
      return `No data came for ${Math.round(failure.afterMs / 1_000)} seconds.`;
    case "network":
      return "The connection broke before the download finished.";
    case "decode":
      return "The device's answer wasn't what was asked for.";
    default:
      return "The download broke.";
  }
}

function refusalReason(failure: ApiFailure): string {
  if (failure.kind === "api") {
    switch (failure.code) {
      case "sensitive":
        return "The region never leaves the device.";
      case "unavailable":
        return "The firmware can't read the region this boot.";
      case "not_found":
        return "The device doesn't have that region.";
      default:
        return `The device refused with the error ${failure.code}.`;
    }
  }
  if (failure.kind === "unauthorized") return "The session isn't signed in.";
  if (failure.kind === "forbidden") return "This account can't download from the device.";
  return "The device refused the download.";
}

export function createDownloads(deps: DownloadsDeps): Downloads {
  const { timers } = deps;
  const entries = signal<readonly DownloadEntry[]>([]);
  const runtimes = new Map<number, Runtime>();
  let nextId = 1;
  let running = false;

  const pending = computed(() =>
    entries.value.some(
      (entry) =>
        !TERMINAL.includes(entry.state.kind) || (entry.state.kind === "failed" && entry.state.resumable),
    ),
  );
  pending.subscribe((on) => {
    deps.guardUnload(on);
  });

  function find(id: number): DownloadEntry | undefined {
    return entries.peek().find((entry) => entry.id === id);
  }

  function update(id: number, patch: Partial<Omit<DownloadEntry, "id">>): void {
    entries.value = entries.peek().map((entry) => (entry.id === id ? { ...entry, ...patch } : entry));
  }

  function setState(id: number, state: EntryState): void {
    update(id, { state });
  }

  function runtime(id: number): Runtime {
    let found = runtimes.get(id);
    if (found === undefined) {
      found = { received: 0, resumes: 0, restarts: 0, cancelled: false };
      runtimes.set(id, found);
    }
    return found;
  }

  function sleep(rt: Runtime, ms: number): Promise<void> {
    return new Promise((done) => {
      const handle = timers.setTimeout(() => {
        delete rt.wake;
        done();
      }, ms);
      rt.wake = () => {
        timers.clearTimeout(handle);
        delete rt.wake;
        done();
      };
    });
  }

  function pump(): void {
    if (running) return;
    const next = entries.peek().find((entry) => entry.state.kind === "queued");
    if (next === undefined) return;
    running = true;
    void run(next.id).finally(() => {
      running = false;
      pump();
    });
  }

  function fail(id: number, rt: Runtime, reason: string, resumable: boolean, received = rt.received): void {
    if (!resumable) {
      runtimes.delete(id);
      delete rt.buffer;
    }
    setState(id, { kind: "failed", reason, resumable, received });
  }

  async function run(id: number): Promise<void> {
    const rt = runtime(id);
    rt.cancelled = false;
    let needsPlan = find(id)?.plan === undefined || rt.buffer === undefined;
    for (;;) {
      const entry = find(id);
      if (entry === undefined || rt.cancelled) return;

      if (needsPlan) {
        setState(id, { kind: "downloading", received: 0, total: undefined, retrying: false });
        const planned = await deps.prepare(entry.target);
        if (rt.cancelled) return;
        if (!planned.ok) {
          if (planned.transient === true && rt.resumes < MAX_RESUMES) {
            await sleep(rt, backoffDelayMs(rt.resumes, deps.random, RESUME_DELAYS_MS));
            rt.resumes += 1;
            continue;
          }
          fail(id, rt, planned.reason, planned.transient === true, 0);
          return;
        }
        update(id, { plan: planned.plan });
        rt.buffer = new Uint8Array(planned.plan.size);
        rt.received = 0;
        delete rt.etag;
        needsPlan = false;
      }

      const plan = find(id)?.plan;
      const buffer = rt.buffer;
      if (plan === undefined || buffer === undefined) return;
      if (rt.received > 0 && rt.etag === undefined) rt.received = 0;
      const start = plan.span?.first ?? 0;
      const last = plan.span?.last ?? plan.size - 1;
      const resuming = rt.received > 0;
      const headers: { Range?: string; "If-Range"?: string } = {};
      if (resuming) {
        headers.Range = `bytes=${start + rt.received}-${last}`;
        if (rt.etag !== undefined) headers["If-Range"] = rt.etag;
      } else if (plan.span !== undefined) {
        headers.Range = `bytes=${plan.span.first}-${plan.span.last}`;
      }
      setState(id, { kind: "downloading", received: rt.received, total: plan.size, retrying: rt.resumes > 0 });

      let problem: "changed" | "unexpected" | "overflow" | undefined;
      let published = deps.now();
      const call = deps.api.download("GetRegionContent", {
        params: { region: plan.region },
        headers,
        onHead(head: DownloadHead) {
          if (head.status === 200) {
            // A resume answered whole means the region changed; a part asked for and answered whole is unexpected.
            if (resuming) problem = "changed";
            else if (plan.span !== undefined || (head.length !== undefined && head.length !== plan.size)) {
              problem = "unexpected";
            }
          } else if (
            head.range === undefined ||
            head.range.first !== start + rt.received ||
            head.range.last !== last ||
            (resuming && head.etag !== rt.etag)
          ) {
            problem = "unexpected";
          }
          if (problem !== undefined) {
            call.cancel();
            return;
          }
          if (!resuming) {
            if (head.etag === undefined) delete rt.etag;
            else rt.etag = head.etag;
          }
        },
        onChunk(chunk: Uint8Array) {
          if (rt.received + chunk.byteLength > buffer.byteLength) {
            problem = "overflow";
            call.cancel();
            return;
          }
          buffer.set(chunk, rt.received);
          rt.received += chunk.byteLength;
          const now = deps.now();
          if (now - published >= PROGRESS_MS) {
            published = now;
            setState(id, { kind: "downloading", received: rt.received, total: plan.size, retrying: false });
          }
        },
      });
      rt.call = call;
      const outcome = await call.done;
      delete rt.call;
      if (rt.cancelled) return;

      if (problem === "changed" || problem === "unexpected" || problem === "overflow") {
        rt.restarts += 1;
        if (rt.restarts > MAX_RESTARTS) {
          fail(id, rt, "The region kept changing while it downloaded.", false);
          return;
        }
        if (problem === "changed") {
          update(id, { note: "The region changed since the download started, so it started again." });
        }
        needsPlan = true;
        continue;
      }

      if (outcome.ok && rt.received === plan.size) {
        await finish(id, rt, plan, buffer);
        return;
      }

      const failure: ApiFailure = outcome.ok ? { kind: "network" } : outcome.failure;
      if (failure.kind === "busy") {
        const waitS = Math.min(MAX_BUSY_WAIT_S, Math.max(1, failure.retryAfterS ?? BUSY_WAIT_S));
        const since = deps.now();
        setState(id, { kind: "waiting", since, until: since + waitS * 1_000 });
        await sleep(rt, waitS * 1_000);
        continue;
      }
      if (failure.kind === "api" && failure.code === "range_not_satisfiable") {
        rt.restarts += 1;
        if (rt.restarts > MAX_RESTARTS) {
          fail(id, rt, "The range is outside the region.", false);
          return;
        }
        update(id, { note: "The region changed size since the download started, so it started again." });
        needsPlan = true;
        continue;
      }
      if (failure.kind === "network" || failure.kind === "timeout" || failure.kind === "decode") {
        if (rt.resumes < MAX_RESUMES) {
          setState(id, { kind: "downloading", received: rt.received, total: plan.size, retrying: true });
          await sleep(rt, backoffDelayMs(rt.resumes, deps.random, RESUME_DELAYS_MS));
          rt.resumes += 1;
          continue;
        }
        fail(id, rt, brokenReason(failure), true);
        return;
      }
      fail(id, rt, refusalReason(failure), false);
      return;
    }
  }

  async function finish(id: number, rt: Runtime, plan: DownloadPlan, buffer: Uint8Array): Promise<void> {
    const { check } = plan;
    const hash = deps.sha256;
    if (check !== undefined && hash !== undefined) {
      setState(id, { kind: "verifying" });
      const actual = await hash(buffer.subarray(0, check.bytes));
      if (rt.cancelled) return;
      if (actual !== check.sha256) {
        // The bytes stay for saving anyway, under a name that says so.
        setState(id, {
          kind: "failed",
          reason:
            check.against === "file_sha256"
              ? "The download doesn't match the image's SHA-256, so it wasn't saved."
              : "The image in the download doesn't match its stored digest, so it wasn't saved.",
          resumable: false,
          received: rt.received,
          mismatch: { expected: check.sha256, actual },
        });
        return;
      }
    }
    deps.save(buffer, plan.filename);
    runtimes.delete(id);
    delete rt.buffer;
    setState(id, { kind: "done", verified: check !== undefined && hash !== undefined ? "checked" : "unchecked" });
  }

  return {
    entries,
    pending,
    add(target) {
      const id = nextId++;
      entries.value = [...entries.peek(), { id, target, queuedAt: deps.now(), state: { kind: "queued" } }];
      pump();
      return id;
    },
    act(id, action) {
      const entry = find(id);
      if (entry === undefined) return;
      const rt = runtimes.get(id);
      switch (action) {
        case "cancel":
          if (TERMINAL.includes(entry.state.kind)) return;
          if (rt !== undefined) {
            rt.cancelled = true;
            rt.call?.cancel();
            rt.wake?.();
            delete rt.buffer;
          }
          runtimes.delete(id);
          setState(id, { kind: "cancelled" });
          return;
        case "retry": {
          const { state } = entry;
          if (state.kind !== "failed" && state.kind !== "cancelled") return;
          const queued = { id, target: entry.target, queuedAt: deps.now(), state: { kind: "queued" } as const };
          // The array stays in queue order: the entry goes to its end, as a new download would.
          const rest = entries.peek().filter((item) => item.id !== id);
          if (state.kind === "failed" && state.resumable && rt !== undefined) {
            rt.resumes = 0;
            entries.value = [...rest, entry.plan === undefined ? queued : { ...queued, plan: entry.plan }];
          } else {
            // The bytes kept for saving anyway go, and the download is planned again.
            runtimes.delete(id);
            entries.value = [...rest, queued];
          }
          pump();
          return;
        }
        case "save-anyway": {
          const buffer = rt?.buffer;
          if (entry.state.kind !== "failed" || entry.state.mismatch === undefined || buffer === undefined) return;
          deps.save(buffer, unverifiedFilename(entry.plan?.filename ?? "download.bin"));
          runtimes.delete(id);
          setState(id, { kind: "done", verified: "mismatch" });
          return;
        }
        case "dismiss":
          if (!TERMINAL.includes(entry.state.kind)) return;
          runtimes.delete(id);
          entries.value = entries.peek().filter((item) => item.id !== id);
          return;
      }
    },
  };
}
