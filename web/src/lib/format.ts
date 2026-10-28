const SECOND = 1_000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** An uptime in seconds as its two largest units, such as `1d 3h`. */
export function formatUptime(seconds: number): string {
  const d = Math.floor(seconds / 86_400);
  const h = Math.floor((seconds % 86_400) / 3_600);
  const m = Math.floor((seconds % 3_600) / 60);
  const s = Math.floor(seconds % 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

/** A countdown in seconds as `m:ss`. */
export function formatCountdown(seconds: number): string {
  const total = Math.max(0, Math.ceil(seconds));
  const m = Math.floor(total / 60);
  const s = String(total % 60).padStart(2, "0");
  return `${m}:${s}`;
}

/** How long ago something was, such as `12 seconds ago` or `2 minutes ago`. */
export function formatAge(ageMs: number, locale?: string): string {
  const formatter = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  const age = Math.max(0, ageMs);
  if (age < 5 * SECOND) return formatter.format(0, "second");
  if (age < MINUTE) return formatter.format(-Math.floor(age / SECOND), "second");
  if (age < HOUR) return formatter.format(-Math.floor(age / MINUTE), "minute");
  if (age < DAY) return formatter.format(-Math.floor(age / HOUR), "hour");
  return formatter.format(-Math.floor(age / DAY), "day");
}
