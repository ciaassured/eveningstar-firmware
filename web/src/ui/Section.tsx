import type { ComponentChildren } from "preact";
import { useId } from "preact/hooks";
import styles from "./section.module.css";

/** A titled panel of the inspector, such as "Usage summary". */
export function Section({ title, after, children }: { title: string; after?: ComponentChildren; children: ComponentChildren }) {
  const id = useId();
  return (
    <section class={styles["section"]} aria-labelledby={id}>
      <h3 class={styles["title"]}>
        <span id={id}>{title}</span>
        {after}
      </h3>
      {children}
    </section>
  );
}
