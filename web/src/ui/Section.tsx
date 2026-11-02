import type { ComponentChildren } from "preact";
import { useId } from "preact/hooks";
import { cx } from "../lib/cx.ts";
import styles from "./section.module.css";

/** A titled panel of the inspector, such as "Usage summary". With `stack`, its parts are 10 px apart. */
export function Section({
  title,
  after,
  stack,
  children,
}: {
  title: string;
  after?: ComponentChildren;
  stack?: boolean;
  children: ComponentChildren;
}) {
  const id = useId();
  return (
    <section class={cx(styles["section"], stack === true && styles["stack"])} aria-labelledby={id}>
      <h3 class={styles["title"]}>
        <span id={id}>{title}</span>
        {after}
      </h3>
      {children}
    </section>
  );
}
