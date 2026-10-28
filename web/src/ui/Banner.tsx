import type { ComponentChildren } from "preact";
import { cx } from "../lib/cx.ts";
import { StatusIcon } from "./StatusIcon.tsx";
import type { StatusLevel } from "./StatusIcon.tsx";
import styles from "./banner.module.css";

/**
 * Something that needs attention: a bar in the colour of its level on the
 * left, and the level's shape and word before the message.
 */
export function Banner({
  level,
  title,
  urgent = false,
  children,
}: {
  level: StatusLevel;
  title: string;
  /** Whether to interrupt assistive technology when it appears. */
  urgent?: boolean;
  children?: ComponentChildren;
}) {
  return (
    <div class={cx(styles["banner"], styles[level])} role={urgent ? "alert" : "status"}>
      <p class={styles["title"]}>
        <StatusIcon level={level} />
        <span>{title}</span>
      </p>
      {children}
    </div>
  );
}
