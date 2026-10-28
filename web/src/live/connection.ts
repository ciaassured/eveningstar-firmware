import type { ReadonlySignal } from "@preact/signals";
import { signal } from "@preact/signals";

/**
 * Whether the device answers: `online`, `degraded` when the latest request
 * failed and the page is retrying, `offline` when requests have been failing
 * for a while.
 */
export type ConnectionState = "online" | "degraded" | "offline";

/** Requests failing for this long mean the device is offline. */
export const OFFLINE_AFTER_MS = 15_000;
/** This many failures in a row mean the device is offline. */
export const OFFLINE_AFTER_FAILURES = 3;

export interface Connection {
  readonly state: ReadonlySignal<ConnectionState>;
  /** Records whether the device answered the latest request. */
  report(reachable: boolean): void;
}

export function createConnection(now: () => number): Connection {
  const state = signal<ConnectionState>("online");
  let failures = 0;
  let failingSince = 0;

  return {
    state,
    report(reachable) {
      if (reachable) {
        failures = 0;
        state.value = "online";
        return;
      }
      if (failures === 0) failingSince = now();
      failures += 1;
      const long = now() - failingSince >= OFFLINE_AFTER_MS;
      state.value = long || failures >= OFFLINE_AFTER_FAILURES ? "offline" : "degraded";
    },
  };
}
