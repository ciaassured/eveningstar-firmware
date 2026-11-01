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

const BINARY_UNITS = ["B", "KiB", "MiB", "GiB"] as const;

/** A size in binary units, such as `32 KiB` or `1.15 MiB`. */
export function formatBytes(bytes: number, locale?: string): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < BINARY_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 ? 0 : value < 10 ? 2 : value < 100 ? 1 : 0;
  const number = new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(value);
  return `${number} ${BINARY_UNITS[unit] ?? ""}`;
}

/** An exact count of bytes, such as `1,203,456 bytes`. */
export function formatExactBytes(bytes: number, locale?: string): string {
  return `${new Intl.NumberFormat(locale).format(bytes)} ${bytes === 1 ? "byte" : "bytes"}`;
}

/** An offset in hexadecimal, padded to `digits`, such as `0x010000`. */
export function formatHex(value: number, digits = 6): string {
  return `0x${value.toString(16).padStart(digits, "0")}`;
}

/** A digest shortened to its first 8 and last 4 digits. */
export function shortDigest(digest: string): string {
  return digest.length <= 16 ? digest : `${digest.slice(0, 8)}…${digest.slice(-4)}`;
}

/** A share in [0, 1] as a whole percentage, rounded down so that 100% means all of it. */
export function formatPercent(share: number): string {
  return `${Math.floor(Math.max(0, Math.min(1, share)) * 100)}%`;
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
