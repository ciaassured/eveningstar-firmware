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

/** An offset in hexadecimal, padded to `digits`, such as `0x010000`. */
export function formatHex(value: number, digits = 6): string {
  return `0x${value.toString(16).padStart(digits, "0")}`;
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

/** A time of day on the 24-hour clock with seconds, such as `09:05:07`. */
export function formatClock(ms: number): string {
  const time = new Date(ms);
  return [time.getHours(), time.getMinutes(), time.getSeconds()].map((part) => String(part).padStart(2, "0")).join(":");
}

/** A whole number grouped by thousands with commas, such as `3,211,264`. */
export function formatDecimal(value: number): string {
  return new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 }).format(value);
}

/** A count of bytes in text, such as `1,152,096 bytes`. */
export function formatByteCount(bytes: number): string {
  return `${formatDecimal(bytes)} bytes`;
}

const TWO_DECIMALS = new Intl.NumberFormat("en-US", { maximumFractionDigits: 2, useGrouping: false });

/**
 * A size in binary units with up to two decimals and no trailing zeros: MiB
 * from 1 MiB (`3 MiB`, `1.13 MiB`), KiB from 1 KiB (`22.14 KiB`), else bytes
 * (`32 bytes`).
 */
export function formatSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${TWO_DECIMALS.format(bytes / (1024 * 1024))} MiB`;
  if (bytes >= 1024) return `${TWO_DECIMALS.format(bytes / 1024)} KiB`;
  return formatByteCount(bytes);
}

/** A share as a percentage with one decimal, such as `37.6%`. */
export function formatPercent1(share: number): string {
  return `${(Math.round(share * 1000) / 10).toFixed(1)}%`;
}

/** An offset as a range shows it: the hex, and the decimal beside it. */
export interface OffsetText {
  /** `0x` and at least six lowercase digits, such as `0x010000`. */
  hex: string;
  /** Grouped with commas, such as `65,536`. */
  decimal: string;
}

/** A size as a range shows it. */
export interface SizeText {
  /** `0x` and at least six lowercase digits, such as `0x300000`. */
  hex: string;
  /** The bytes, and from 1 KiB the size in KiB or MiB: `3,145,728 bytes, 3 MiB`, `32 bytes`. */
  text: string;
  /** The size alone, such as `3 MiB`. */
  human: string;
}

/** A range in words: its start, its end (the first byte after it) and its size. */
export interface SpanText {
  start: OffsetText;
  /** End, exclusive. */
  end: OffsetText;
  size: SizeText;
}

/** An offset as a range shows it. */
export function formatOffset(offset: number): OffsetText {
  return { hex: formatHex(offset), decimal: formatDecimal(offset) };
}

/** A size as a range shows it. */
export function formatSizeText(size: number): SizeText {
  const human = formatSize(size);
  return {
    hex: formatHex(size),
    text: size >= 1024 ? `${formatByteCount(size)}, ${human}` : formatByteCount(size),
    human,
  };
}

/** The range from `offset` up to, but not including, `offset + size`. */
export function formatSpan(offset: number, size: number): SpanText {
  return { start: formatOffset(offset), end: formatOffset(offset + size), size: formatSizeText(size) };
}

/** Words joined as a list, such as `offset and size` or `type, offset and size`. */
export function joinWords(words: readonly string[]): string {
  if (words.length <= 1) return words[0] ?? "";
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1] ?? ""}`;
}
