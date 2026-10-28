import type { ComponentChildren } from "preact";
import { useId } from "preact/hooks";
import { cx } from "../lib/cx.ts";
import styles from "./panel.module.css";

/** A titled block on the page. */
export function Panel({
  title,
  wide,
  stale,
  children,
}: {
  title: string;
  wide?: boolean;
  /** Whether what it shows is out of date. */
  stale?: boolean;
  children: ComponentChildren;
}) {
  const id = useId();
  return (
    <section class={cx(styles["panel"], wide && styles["wide"], stale && styles["stale"])} aria-labelledby={id}>
      <h2 class={styles["title"]} id={id}>
        {title}
      </h2>
      {children}
    </section>
  );
}

/** The panels of a page, in a grid. */
export function PanelGrid({ children }: { children: ComponentChildren }) {
  return <div class={styles["grid"]}>{children}</div>;
}

/** A labelled key figure. */
export function Metric({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div class={styles["metric"]}>
      <div class={styles["label"]}>{label}</div>
      <div class={cx(styles["figure"], mono && styles["mono"])}>{value}</div>
    </div>
  );
}

/** How old a live value is. It turns to a warning colour once the value is stale. */
export function Age({ text, stale }: { text: string; stale: boolean }) {
  return <p class={cx(styles["age"], stale && styles["old"])}>{stale ? `Stale: updated ${text}` : `Updated ${text}`}</p>;
}
