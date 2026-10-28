import type { ReadonlySignal } from "@preact/signals";
import { signal } from "@preact/signals";
import type { Result } from "../api/client.ts";
import type { ApiFailure, FailureContext } from "../api/failure.ts";
import { isTransient } from "../api/failure.ts";
import type { Timers } from "../api/transport.ts";

/** Loads one thing a view shows. */
export type Loader = () => Promise<Result<unknown>>;

export type LoadState =
  | { kind: "loading" }
  | { kind: "ready" }
  | {
      kind: "failed";
      failure: ApiFailure;
      context: FailureContext;
      /** Seconds until the next automatic attempt; `undefined` if there won't be one. */
      retryInS?: number;
    };

/** The seconds before each automatic attempt after a transient failure. */
export const AUTO_RETRY_DELAYS_S: readonly number[] = [3, 6, 12, 24, 30];

export interface LoadControllerDeps {
  loaders: readonly Loader[];
  timers: Timers;
  /** Aborts what the view has queued; called when the view is left. */
  abort(): void;
}

export interface LoadController {
  readonly state: ReadonlySignal<LoadState>;
  /** Runs the loaders in order. */
  load(): Promise<void>;
  retry(): void;
  dispose(): void;
}

/**
 * Loads everything a view needs before it shows anything: it is ready only
 * when every loader succeeded, and otherwise shows why not. Transient failures
 * are tried again after a countdown.
 */
export function createLoadController(deps: LoadControllerDeps): LoadController {
  const { timers } = deps;
  const state = signal<LoadState>({ kind: "loading" });
  let timer: unknown;
  let failures = 0;
  let disposed = false;
  let generation = 0;

  function clear(): void {
    timers.clearTimeout(timer);
  }

  function countdown(seconds: number, failure: ApiFailure, context: FailureContext): void {
    state.value = { kind: "failed", failure, context, retryInS: seconds };
    timer = timers.setTimeout(() => {
      if (seconds > 1) countdown(seconds - 1, failure, context);
      else void load();
    }, 1_000);
  }

  async function load(): Promise<void> {
    clear();
    generation += 1;
    const mine = generation;
    state.value = { kind: "loading" };
    for (const loader of deps.loaders) {
      const result = await loader();
      if (disposed || mine !== generation) return;
      if (!result.ok) {
        if (result.failure.kind === "aborted") return;
        const delay = AUTO_RETRY_DELAYS_S[Math.min(failures, AUTO_RETRY_DELAYS_S.length - 1)] ?? 30;
        failures += 1;
        if (isTransient(result.failure)) countdown(delay, result.failure, result.context);
        else state.value = { kind: "failed", failure: result.failure, context: result.context };
        return;
      }
    }
    failures = 0;
    state.value = { kind: "ready" };
  }

  return {
    state,
    load,
    retry() {
      void load();
    },
    dispose() {
      disposed = true;
      clear();
      deps.abort();
    },
  };
}
