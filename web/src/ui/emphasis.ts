import type { RefObject } from "preact";
import { useEffect } from "preact/hooks";

/**
 * Emphasis of what is pointed at or focused, without rendering again. An
 * element that stands for a region carries `data-region` with the region's
 * key; one that stands for a part of a region carries `data-part`; the
 * readout of a region carries `data-readout`. While a region is emphasised,
 * the root carries `data-emphasised`, and each element of the region carries
 * `data-hl`; each element of an emphasised part carries `data-phl`. Nothing
 * inside an element with `data-no-emphasis`, such as a popover, emphasises.
 */
export function useEmphasis(root: RefObject<HTMLElement | null>): void {
  useEffect(() => {
    let region: string | undefined;
    let part: string | undefined;

    const mark = (attribute: string, selector: string | undefined): void => {
      const element = root.current;
      if (element === null) return;
      for (const marked of element.querySelectorAll(`[${attribute}]`)) marked.removeAttribute(attribute);
      if (selector === undefined) return;
      for (const target of element.querySelectorAll(selector)) target.setAttribute(attribute, "");
    };
    const setRegion = (next: string | undefined): void => {
      if (next === region) return;
      region = next;
      const value = next === undefined ? undefined : CSS.escape(next);
      mark("data-hl", value === undefined ? undefined : `[data-region="${value}"], [data-readout="${value}"]`);
      if (next === undefined) root.current?.removeAttribute("data-emphasised");
      else root.current?.setAttribute("data-emphasised", "");
    };
    const setPart = (next: string | undefined): void => {
      if (next === part) return;
      part = next;
      mark("data-phl", next === undefined ? undefined : `[data-part="${CSS.escape(next)}"]`);
    };
    const from = (target: EventTarget | null): void => {
      const element = target instanceof Element ? target : null;
      if (element === null || element.closest("[data-no-emphasis]") !== null || root.current?.contains(element) !== true) {
        setRegion(undefined);
        setPart(undefined);
        return;
      }
      setRegion(element.closest("[data-region]")?.getAttribute("data-region") ?? undefined);
      setPart(element.closest("[data-part]")?.getAttribute("data-part") ?? undefined);
    };
    const over = (event: PointerEvent): void => {
      if (event.pointerType !== "touch") from(event.target);
    };
    const focus = (event: FocusEvent): void => {
      from(event.target);
    };
    const leave = (): void => {
      setRegion(undefined);
      setPart(undefined);
    };
    document.addEventListener("pointerover", over);
    document.addEventListener("focusin", focus);
    document.documentElement.addEventListener("pointerleave", leave);
    return () => {
      document.removeEventListener("pointerover", over);
      document.removeEventListener("focusin", focus);
      document.documentElement.removeEventListener("pointerleave", leave);
      leave();
    };
  }, [root]);
}
