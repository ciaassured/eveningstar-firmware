import { formatHex, formatSize } from "../lib/format.ts";

/** The share every block gets at least, in percent of the strip, while the blocks are few. */
export const BLOCK_MINIMUM = 6;
/** The most of the strip the minimum shares take together, in percent. */
export const MINIMUMS_MAX = 60;

/** The width of the strip's blocks. */
export interface StripWeights {
  /** Each block's `flex-grow`. */
  grow: number[];
  /** Each block's share of the strip's width; they add up to 1. */
  share: number[];
  /** The minimum each block's `grow` starts from: 6, or `60 / n` with more than 10 blocks. */
  minimum: number;
}

/**
 * The strip's block widths for blocks of `sizes` on a strip of `total` bytes:
 * `grow = m + (100 − m·n) × size / total`, with `m` 6, or `60 / n` once `6n`
 * exceeds 60. The weights are relative, so overlapping blocks whose sizes add
 * up to more than the total still fill the strip exactly.
 */
export function stripWeights(sizes: readonly number[], total: number): StripWeights {
  const n = sizes.length;
  if (n === 0) return { grow: [], share: [], minimum: BLOCK_MINIMUM };
  const minimum = BLOCK_MINIMUM * n > MINIMUMS_MAX ? MINIMUMS_MAX / n : BLOCK_MINIMUM;
  const rest = 100 - minimum * n;
  const grow = sizes.map((size) => minimum + (total > 0 ? (rest * Math.max(0, size)) / total : 0));
  const sum = grow.reduce((a, b) => a + b, 0);
  return { grow, share: grow.map((weight) => weight / sum), minimum };
}

/** What a strip block, a readout or a tray swatch names. */
export interface Named {
  title: string;
  subtitle: string;
  span: { start: number; end: number; size: number };
  status: { word: string };
}

/** A block's accessible name: `<title>, <subtitle>. <start> to <end>, <size>. <status word>.` */
export function blockName(item: Named): string {
  return `${item.title}, ${item.subtitle}. ${formatHex(item.span.start)} to ${formatHex(item.span.end)}, ${formatSize(item.span.size)}. ${item.status.word}.`;
}

/** The readout line's centre for a region. */
export interface Readout {
  title: string;
  subtitle: string;
  start: string;
  end: string;
  size: string;
}

export function readout(item: Named): Readout {
  const start = formatHex(item.span.start);
  const end = formatHex(item.span.end);
  const size = formatSize(item.span.size);
  return { title: item.title, subtitle: item.subtitle, start, end, size };
}

/**
 * The part of a region its usage bar on the strip stands for, as the
 * inspector's ruler and span table key it: the image of an app slot or the
 * bootloader, the partition table's entries, the otadata's first record.
 */
export function usagePart(region: string): string | undefined {
  switch (region) {
    case "bootloader":
    case "app_current":
    case "app_alternate":
      return "image";
    case "partition_table":
      return "entries";
    case "otadata":
      return "sector0";
    default:
      return undefined;
  }
}

/** The readout line's ends: the strip's first offset, and its end and size. */
export function readoutEnds(stripEnd: number): { start: string; end: string } {
  return { start: formatHex(0), end: `${formatHex(stripEnd)}, ${formatSize(stripEnd)}` };
}
