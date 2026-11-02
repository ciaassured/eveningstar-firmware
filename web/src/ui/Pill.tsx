import { useRef } from "preact/hooks";
import { cx } from "../lib/cx.ts";
import { Icon } from "./Icon.tsx";
import styles from "./pill.module.css";
import { togglePopover, usePopoverOpen } from "./Popover.tsx";
import type { Level } from "./region.ts";
import { StatusLine } from "./StatusMark.tsx";

/**
 * A status pill: the icon and the word in the level's colour, then "Why?".
 * Pressing it opens the Why popover: the word and the reason, then a note.
 */
export function StatusPill({
  level,
  word,
  name,
  why,
}: {
  level: Level;
  word: string;
  /** The accessible name, such as "Differs. Show why". */
  name: string;
  why: { word: string; reason: string; note: string };
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const expanded = usePopoverOpen(ref);
  return (
    <button
      ref={ref}
      type="button"
      class={cx(styles["pill"], styles[level])}
      aria-label={name}
      aria-expanded={expanded}
      onClick={() => {
        if (ref.current === null) return;
        togglePopover(
          ref.current,
          <>
            <StatusLine level={level}>
              <b>{why.word}.</b> {why.reason}
            </StatusLine>
            <p class={styles["note"]}>{why.note}</p>
          </>,
        );
      }}
    >
      <Icon name={level} />
      {word}
      <span class={styles["why"]}>Why?</span>
    </button>
  );
}
