/** A box on the screen, in CSS pixels. */
export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** How far a popover keeps from the viewport's sides, from its top and bottom, and from its trigger. */
export const SIDE_MARGIN = 16;
export const END_MARGIN = 8;
export const TRIGGER_GAP = 6;

/**
 * Where a popover of `size` goes for a trigger at `trigger` in a viewport of
 * `viewport`: under the trigger, aligned to its left edge and kept 16 px from
 * the viewport's sides; above it when it doesn't fit below and does above;
 * never closer than 8 px to the top or bottom.
 */
export function placePopover(
  trigger: Box,
  size: { width: number; height: number },
  viewport: { width: number; height: number },
): { left: number; top: number } {
  const left = Math.max(SIDE_MARGIN, Math.min(trigger.left, viewport.width - size.width - SIDE_MARGIN));
  const bottom = trigger.top + trigger.height;
  let top = bottom + TRIGGER_GAP;
  const above = trigger.top - size.height - TRIGGER_GAP;
  if (top + size.height > viewport.height - END_MARGIN && above > END_MARGIN) top = above;
  top = Math.max(END_MARGIN, Math.min(top, viewport.height - size.height - END_MARGIN));
  return { left, top };
}

/**
 * The index focus moves to from `index` in a list of `count` items on `key`:
 * one down on ArrowDown, one up on ArrowUp, staying at the ends; `undefined`
 * for any other key, or when there is nowhere to go.
 */
export function stepIndex(index: number, count: number, key: string): number | undefined {
  const next = key === "ArrowDown" ? index + 1 : key === "ArrowUp" ? index - 1 : undefined;
  return next === undefined || next < 0 || next >= count ? undefined : next;
}

/** How close to the top, in CSS pixels, a list counts as scrolled to the top. */
export const TOP_SLACK = 2;

/**
 * Where a list scrolled to `top` goes when items `added` pixels tall are put
 * above what it shows: at the top it stays, and the new items show; scrolled
 * away it moves down by them, keeping what it shows, and `pill` says to point
 * at them while the list is `open`.
 */
export function keepPlace(top: number, added: number, open: boolean): { top: number; pill: boolean } {
  return top <= TOP_SLACK ? { top, pill: false } : { top: top + added, pill: open };
}
