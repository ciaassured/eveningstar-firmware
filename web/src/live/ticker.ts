import type { ReadonlySignal } from "@preact/signals";
import { signal } from "@preact/signals";
import type { Timers } from "../api/transport.ts";

/** The time, in milliseconds since the epoch, refreshed every second while `active`. */
export function createTicker(
  timers: Timers,
  now: () => number,
  active: ReadonlySignal<boolean>,
): ReadonlySignal<number> {
  const time = signal(now());
  let timer: unknown;

  function tick(): void {
    time.value = now();
    timer = timers.setTimeout(tick, 1_000);
  }

  active.subscribe((isActive) => {
    timers.clearTimeout(timer);
    if (isActive) tick();
  });
  return time;
}
