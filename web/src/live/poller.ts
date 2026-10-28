import type { ReadonlySignal } from "@preact/signals";
import { batch, signal } from "@preact/signals";
import type { Result } from "../api/client.ts";
import type { ApiFailure } from "../api/failure.ts";
import type { Priority, Timers } from "../api/transport.ts";
import { widen } from "../lib/backoff.ts";

/** A value of the device that stays up to date while the page is in use. */
export interface LiveSource<T> {
  readonly value: ReadonlySignal<T | undefined>;
  /** When `value` was fetched, in milliseconds since the epoch. */
  readonly updatedAt: ReadonlySignal<number | undefined>;
  /** Why the latest fetch failed; `undefined` if it didn't. */
  readonly error: ReadonlySignal<ApiFailure | undefined>;
  /** The normal interval between fetches. */
  readonly intervalMs: number;
  /**
   * Fetches now, as `priority` asks, and updates the signals with the result.
   * Leaving the page `group` names aborts the request.
   */
  load(priority: Priority, group?: string): Promise<Result<T>>;
  /** Forgets the value and its error, such as when another person signs in. */
  reset(): void;
  /** Starts fetching while the page is in use. */
  start(): void;
  stop(): void;
}

export interface PollerDeps<T> {
  fetch(priority: Priority, group?: string): Promise<Result<T>>;
  intervalMs: number;
  /** The interval fetches back off to while they fail. */
  maxIntervalMs: number;
  timers: Timers;
  now: () => number;
  /** Whether the page is in use: nothing is fetched while it isn't. */
  active: ReadonlySignal<boolean>;
}

/** A live source that polls. */
export function createPoller<T>(deps: PollerDeps<T>): LiveSource<T> {
  const { timers } = deps;
  const value = signal<T | undefined>(undefined);
  const updatedAt = signal<number | undefined>(undefined);
  const error = signal<ApiFailure | undefined>(undefined);
  let interval = deps.intervalMs;
  let timer: unknown;
  let unsubscribe: (() => void) | undefined;
  let running = false;

  async function load(priority: Priority, group?: string): Promise<Result<T>> {
    const result = await deps.fetch(priority, group);
    if (result.ok) {
      interval = deps.intervalMs;
      batch(() => {
        value.value = result.value;
        updatedAt.value = deps.now();
        error.value = undefined;
      });
    } else if (result.failure.kind !== "aborted") {
      interval = widen(interval, deps.maxIntervalMs);
      error.value = result.failure;
    }
    return result;
  }

  async function tick(): Promise<void> {
    if (running) return;
    running = true;
    try {
      await load("poll");
    } finally {
      running = false;
    }
    if (unsubscribe !== undefined && deps.active.peek()) schedule();
  }

  function schedule(): void {
    timers.clearTimeout(timer);
    timer = timers.setTimeout(() => {
      void tick();
    }, interval);
  }

  return {
    value,
    updatedAt,
    error,
    intervalMs: deps.intervalMs,
    load,
    reset() {
      batch(() => {
        value.value = undefined;
        updatedAt.value = undefined;
        error.value = undefined;
      });
    },
    start() {
      if (unsubscribe !== undefined) return;
      unsubscribe = deps.active.subscribe((active) => {
        timers.clearTimeout(timer);
        if (active) void tick();
      });
    },
    stop() {
      unsubscribe?.();
      unsubscribe = undefined;
      timers.clearTimeout(timer);
    },
  };
}
