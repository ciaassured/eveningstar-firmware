/** A value is stale after this many polling intervals without an update. */
export const STALE_AFTER_INTERVALS = 3;

export interface Freshness {
  ageMs: number;
  stale: boolean;
}

/**
 * How old a live value is, and whether that's too old to present as current.
 * `undefined` if it has never been fetched.
 */
export function freshness(
  updatedAt: number | undefined,
  now: number,
  intervalMs: number,
): Freshness | undefined {
  if (updatedAt === undefined) return undefined;
  const ageMs = Math.max(0, now - updatedAt);
  return { ageMs, stale: ageMs > STALE_AFTER_INTERVALS * intervalMs };
}
