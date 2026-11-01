import type { Span } from "./types.ts";

/** An offset written in decimal or, with `0x`, in hexadecimal; `undefined` if it isn't one. */
export function parseOffset(text: string): number | undefined {
  const trimmed = text.trim().replace(/_/g, "");
  const value = /^0x[0-9a-f]+$/i.test(trimmed)
    ? Number.parseInt(trimmed.slice(2), 16)
    : /^\d+$/.test(trimmed)
      ? Number(trimmed)
      : Number.NaN;
  return Number.isSafeInteger(value) ? value : undefined;
}

export type SpanInput = { ok: true; span: Span } | { ok: false; error: string };

/**
 * The bytes from `from` to `to`, both included, within a region of `size`
 * bytes. An empty `to` means the end of the region.
 */
export function parseSpan(from: string, to: string, size: number): SpanInput {
  const first = parseOffset(from);
  if (first === undefined) return { ok: false, error: "The first offset isn't a number, such as 4096 or 0x1000." };
  const last = to.trim() === "" ? size - 1 : parseOffset(to);
  if (last === undefined) return { ok: false, error: "The last offset isn't a number, such as 8191 or 0x1fff." };
  if (first >= size) return { ok: false, error: `The first offset is past the region's end (0x${(size - 1).toString(16)}).` };
  if (last >= size) return { ok: false, error: `The last offset is past the region's end (0x${(size - 1).toString(16)}).` };
  if (last < first) return { ok: false, error: "The last offset is before the first." };
  return { ok: true, span: { first, last } };
}
