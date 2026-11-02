import type { RefObject } from "preact";
import { useLayoutEffect, useRef } from "preact/hooks";

/**
 * Sets CSS custom properties, such as a block's width, on the element the
 * returned ref is given to. The Content-Security-Policy refuses style
 * attributes in the markup, so the values are set through the CSSOM.
 */
export function useVars<T extends HTMLElement>(vars: Readonly<Record<`--${string}`, string>>): RefObject<T | null> {
  const ref = useRef<T>(null);
  const key = JSON.stringify(vars);
  useLayoutEffect(() => {
    const element = ref.current;
    if (element === null) return;
    for (const [name, value] of Object.entries(vars)) element.style.setProperty(name, value);
    // The values are compared by their text, so a new object with the same values sets nothing.
  }, [key]);
  return ref;
}

/** A share in [0, 1] as a CSS percentage. */
export function percent(share: number): string {
  return `${(Math.max(0, share) * 100).toFixed(3)}%`;
}
