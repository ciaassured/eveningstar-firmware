import type { Timers } from "../api/transport.ts";

/** Lets every promise that can settle without time passing do so. */
export function settle(): Promise<void> {
  return new Promise((done) => {
    setImmediate(done);
  });
}

interface Pending {
  id: number;
  at: number;
  callback: () => void;
}

/** A clock that moves only when told to. */
export class FakeClock implements Timers {
  now = 0;
  #pending: Pending[] = [];
  #next = 0;

  setTimeout(callback: () => void, ms: number): unknown {
    const id = this.#next++;
    this.#pending.push({ id, at: this.now + ms, callback });
    return id;
  }

  clearTimeout(handle: unknown): void {
    this.#pending = this.#pending.filter((timer) => timer.id !== handle);
  }

  /** Timers waiting to fire. */
  get waiting(): number {
    return this.#pending.length;
  }

  /** Moves time forward, firing timers as they fall due and letting what they start run. */
  async advance(ms: number): Promise<void> {
    const end = this.now + ms;
    await settle();
    for (;;) {
      const due = this.#pending.filter((timer) => timer.at <= end).sort((a, b) => a.at - b.at || a.id - b.id)[0];
      if (due === undefined) break;
      this.#pending = this.#pending.filter((timer) => timer !== due);
      this.now = Math.max(this.now, due.at);
      due.callback();
      await settle();
    }
    this.now = end;
    await settle();
  }
}

export function jsonResponse(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", ...headers },
  });
}

export function emptyResponse(status: number, headers: Record<string, string> = {}): Response {
  return new Response(null, { status, headers });
}

type Step = (url: string, init: RequestInit) => Promise<Response> | Response;

/** A `fetch` that answers from a script and records what it was asked. */
export class FakeFetch {
  readonly calls: { url: string; init: RequestInit }[] = [];
  inFlight = 0;
  maxInFlight = 0;
  #script: Step[] = [];

  /** Adds the answers for the next requests, in order. */
  reply(...steps: Step[]): this {
    this.#script.push(...steps);
    return this;
  }

  /** An answer that arrives when `release` is called, or an error when the request is aborted. */
  static deferred(response: Response): { step: Step; release: () => void } {
    let release!: () => void;
    const arrived = new Promise<void>((done) => {
      release = done;
    });
    const step: Step = (_url, init) =>
      new Promise<Response>((resolve, reject) => {
        init.signal?.addEventListener("abort", () => {
          reject(new DOMException("aborted", "AbortError"));
        });
        void arrived.then(() => {
          resolve(response);
        });
      });
    return { step, release };
  }

  /** An answer that never arrives; the request ends only by being aborted. */
  static hang: Step = (_url, init) =>
    new Promise<Response>((_resolve, reject) => {
      init.signal?.addEventListener("abort", () => {
        reject(new DOMException("aborted", "AbortError"));
      });
    });

  static fail: Step = () => Promise.reject(new TypeError("Failed to fetch"));

  readonly fetch = async (url: string, init: RequestInit): Promise<Response> => {
    this.calls.push({ url, init });
    this.inFlight += 1;
    this.maxInFlight = Math.max(this.maxInFlight, this.inFlight);
    try {
      const step = this.#script.shift();
      if (step === undefined) throw new Error(`no answer scripted for ${url}`);
      return await step(url, init);
    } finally {
      this.inFlight -= 1;
    }
  };
}
