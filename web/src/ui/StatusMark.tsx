import type { ComponentChildren } from "preact";
import { cx } from "../lib/cx.ts";
import { Icon } from "./Icon.tsx";
import type { Level } from "./region.ts";
import styles from "./statusmark.module.css";

/** A status as an icon and a word in the level's colour, with the reason as its tooltip. */
export function StatusMark({ level, word, reason }: { level: Level; word: string; reason?: string | undefined }) {
  return (
    <span class={cx(styles["mark"], styles[level])} title={reason}>
      <Icon name={level} size={14} />
      {word}
    </span>
  );
}

/** A line of text after an icon in the level's colour, the text itself in the text colour. */
export function StatusLine({ level, children }: { level: Level | "muted"; children: ComponentChildren }) {
  return (
    <p class={cx(styles["line"], styles[level])}>
      {level === "muted" ? null : <Icon name={level} />}
      <span>{children}</span>
    </p>
  );
}
