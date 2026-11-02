/** What a Copy button did: wrote the text to the clipboard, or selected it for the person to copy. */
export type CopyOutcome = "copied" | "selected";

/**
 * Writes `text` with `write`, or calls `select` where there is no clipboard
 * (`write` undefined, outside a secure context) or it refuses, by throwing or
 * by rejecting. `write` runs before this returns, as the browser lets a page
 * write to the clipboard only while it handles the person's action.
 */
export function copyOrSelect(
  text: string,
  write: ((text: string) => Promise<void>) | undefined,
  select: () => void,
): Promise<CopyOutcome> {
  const fallback = (): CopyOutcome => {
    select();
    return "selected";
  };
  if (write === undefined) return Promise.resolve(fallback());
  try {
    return write(text).then(() => "copied" as const, fallback);
  } catch {
    return Promise.resolve(fallback());
  }
}
