import { signal } from "@preact/signals";
import type { ComponentChildren } from "preact";
import { useEffect, useId, useLayoutEffect, useRef } from "preact/hooks";
import { placePopover } from "../lib/place.ts";
import styles from "./popover.module.css";

interface OpenPopover {
  trigger: HTMLElement;
  content: ComponentChildren;
}

/** The one popover open on the page; opening another closes it. */
const open = signal<OpenPopover | undefined>(undefined);

/** Opens a popover under `trigger`, or closes it when it is the trigger's own. */
export function togglePopover(trigger: HTMLElement, content: ComponentChildren): void {
  open.value = open.peek()?.trigger === trigger ? undefined : { trigger, content };
}

/** Closes the open popover; with `focus`, focus goes back to its trigger. */
export function closePopover(focus = false): void {
  const current = open.peek();
  open.value = undefined;
  if (focus) current?.trigger.focus();
}

/** Whether the popover open is the one of `trigger`. */
export function usePopoverOpen(trigger: { current: HTMLElement | null }): boolean {
  const current = open.value;
  return current !== undefined && current.trigger === trigger.current;
}

/**
 * Where the open popover is drawn: fixed, above everything, placed under its
 * trigger. It closes on a pointer press outside it and its trigger, on
 * Escape, and when anything but the popover scrolls. Put it once in a page.
 */
export function PopoverHost() {
  const current = open.value;
  const ref = useRef<HTMLDivElement>(null);
  const id = useId();

  useLayoutEffect(() => {
    const element = ref.current;
    if (current === undefined || element === null) return;
    current.trigger.setAttribute("aria-controls", id);
    const place = (): void => {
      const box = current.trigger.getBoundingClientRect();
      const root = document.documentElement;
      const at = placePopover(box, { width: element.offsetWidth, height: element.offsetHeight }, { width: root.clientWidth, height: root.clientHeight });
      element.style.setProperty("left", `${at.left}px`);
      element.style.setProperty("top", `${at.top}px`);
    };
    place();
    globalThis.addEventListener("resize", place);
    return () => {
      globalThis.removeEventListener("resize", place);
    };
  }, [current]);

  useEffect(() => {
    if (current === undefined) return;
    const inside = (target: EventTarget | null): boolean =>
      target instanceof Node && (ref.current?.contains(target) === true || current.trigger.contains(target));
    const press = (event: PointerEvent): void => {
      if (!inside(event.target)) closePopover();
    };
    const key = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      // The page's own Escape, which closes the phone's inspector, waits for the next press.
      event.preventDefault();
      closePopover(true);
    };
    const scroll = (event: Event): void => {
      if (!(event.target instanceof Node && ref.current?.contains(event.target) === true)) closePopover();
    };
    document.addEventListener("pointerdown", press, true);
    document.addEventListener("keydown", key, true);
    document.addEventListener("scroll", scroll, true);
    return () => {
      document.removeEventListener("pointerdown", press, true);
      document.removeEventListener("keydown", key, true);
      document.removeEventListener("scroll", scroll, true);
    };
  }, [current]);

  if (current === undefined) return null;
  return (
    <div ref={ref} id={id} class={styles["popover"]} data-no-emphasis="">
      {current.content}
    </div>
  );
}

/** Text in which runs between backticks are set in the monospace font. */
export function CodeText({ text }: { text: string }) {
  return (
    <>
      {text.split("`").map((part, i) =>
        i % 2 === 1 ? (
          <code key={i} class={styles["code"]}>
            {part}
          </code>
        ) : (
          part
        ),
      )}
    </>
  );
}

/** A "?" button that opens a help popover: the title in bold, then the text. */
export function HelpButton({ label, title, text }: { label: string; title: string; text: string }) {
  const ref = useRef<HTMLButtonElement>(null);
  const expanded = usePopoverOpen(ref);
  return (
    <button
      ref={ref}
      type="button"
      class={styles["help"]}
      aria-label={label}
      aria-expanded={expanded}
      onClick={() => {
        if (ref.current === null) return;
        togglePopover(
          ref.current,
          <>
            <p>
              <b>{title}</b>
            </p>
            <p>
              <CodeText text={text} />
            </p>
          </>,
        );
      }}
    >
      ?
    </button>
  );
}
