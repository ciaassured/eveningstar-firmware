/** Delays, in milliseconds, before each retry; the last one repeats. */
export const RETRY_DELAYS_MS: readonly number[] = [500, 1_000, 2_000, 4_000];

/** The jitter applied to a delay: up to this fraction either way. */
const JITTER = 0.25;

/**
 * The delay before retry number `retry` (0 for the first retry), spread by
 * `random`, a value in [0, 1).
 */
export function backoffDelayMs(
  retry: number,
  random: () => number,
  delays: readonly number[] = RETRY_DELAYS_MS,
): number {
  const base = delays[Math.min(retry, delays.length - 1)] ?? 0;
  return Math.round(base * (1 + JITTER * (2 * random() - 1)));
}

/** `interval` doubled, up to `max`. */
export function widen(interval: number, max: number): number {
  return Math.min(interval * 2, max);
}
