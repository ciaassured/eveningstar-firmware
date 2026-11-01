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

export type Step = (url: string, init: RequestInit) => Promise<Response> | Response;

/** How the fake device serves a region's content. */
export interface ContentScript {
  /** The region's tag; answers carry it, and an `If-Range` with another one gets the whole region. */
  etag?: string;
  /** Ends the body with a network error after this many bytes. */
  cutAfter?: number;
  /** Stops sending after this many bytes, without ending the body. */
  stallAfter?: number;
  /** The size of each chunk sent. */
  chunk?: number;
}

/**
 * A step answering a content request for `bytes` as the device does: the
 * whole region with `200`, a `Range` with `206`, and a stale `If-Range` with
 * the whole region again.
 */
export function serveContent(bytes: Uint8Array, script: ContentScript = {}): Step {
  return (_url, init) => {
    const headers = (init.headers ?? {}) as Record<string, string>;
    let first = 0;
    let last = bytes.byteLength - 1;
    let status = 200;
    const range = /^bytes=(\d+)-(\d*)$/.exec(headers["Range"] ?? "");
    const fresh = headers["If-Range"] === undefined || headers["If-Range"] === script.etag;
    if (range !== null && fresh) {
      first = Number(range[1]);
      last = range[2] === "" ? last : Math.min(last, Number(range[2]));
      status = 206;
      if (first > last) {
        return jsonResponse(416, { error: "range_not_satisfiable" }, { "Content-Range": `bytes */${bytes.byteLength}` });
      }
    }
    const body = bytes.subarray(first, last + 1);
    const chunk = script.chunk ?? 1_024;
    let sent = 0;
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        if (script.cutAfter !== undefined && sent >= script.cutAfter) {
          controller.error(new TypeError("network error"));
          return undefined;
        }
        if (script.stallAfter !== undefined && sent >= script.stallAfter) return new Promise<void>(() => {});
        if (sent >= body.byteLength) {
          controller.close();
          return undefined;
        }
        let end = Math.min(body.byteLength, sent + chunk);
        if (script.cutAfter !== undefined) end = Math.min(end, script.cutAfter);
        if (script.stallAfter !== undefined) end = Math.min(end, script.stallAfter);
        controller.enqueue(body.slice(sent, end));
        sent = end;
        return undefined;
      },
    });
    const out: Record<string, string> = {
      "Content-Type": "application/octet-stream",
      "Content-Length": String(body.byteLength),
      "Accept-Ranges": "bytes",
    };
    if (script.etag !== undefined) out["ETag"] = script.etag;
    if (status === 206) out["Content-Range"] = `bytes ${first}-${last}/${bytes.byteLength}`;
    return new Response(stream, { status, headers: out });
  };
}

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
