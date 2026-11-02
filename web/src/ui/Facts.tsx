import type { ComponentChildren } from "preact";
import { cx } from "../lib/cx.ts";
import styles from "./facts.module.css";
import { Dot, Icon } from "./Icon.tsx";
import type { Level } from "./region.ts";

/** Facts as cells in a wrapping row, each a label over a value. */
export function Facts({ children }: { children: ComponentChildren }) {
  return <dl class={styles["facts"]}>{children}</dl>;
}

/**
 * A fact: the label over the value. With `level`, the value starts with the
 * level's icon and its first words in the level's colour (`marked`), and the
 * rest follows after a gap.
 */
export function Fact({
  label,
  level,
  marked,
  children,
}: {
  label: string;
  level?: Level | undefined;
  marked?: ComponentChildren;
  children?: ComponentChildren;
}) {
  return (
    <div class={styles["fact"]}>
      <dt class={styles["label"]}>{label}</dt>
      <dd class={cx(styles["value"], level !== undefined && styles["flex"])}>
        {level === undefined ? null : (
          <span class={cx(styles["mark"], styles[level])}>
            <Icon name={level} size={14} />
            {marked}
          </span>
        )}
        {children}
      </dd>
    </div>
  );
}

/** A role box: whether an app slot has a boot role, or that it is not known. */
interface RoleBoxProps {
  label: string;
  value: "yes" | "no" | "unknown";
  text: string;
  icon?: "booted" | "next" | "rollback" | undefined;
}

function roleIcon(icon: RoleBoxProps["icon"]) {
  return icon === undefined ? null : icon === "booted" ? <Dot /> : <Icon name={icon} size={14} />;
}

/** The three role boxes of an app slot, in equal columns. */
export function RoleBoxes({ boxes }: { boxes: readonly RoleBoxProps[] }) {
  return (
    <dl class={styles["roles"]}>
      {boxes.map((box) => (
        <div key={box.label} class={cx(styles["role"], box.value === "yes" && styles["on"])}>
          <dt class={styles["rolelabel"]}>{box.label}</dt>
          <dd class={styles["rolevalue"]}>
            {box.value === "yes" ? roleIcon(box.icon) : null}
            {box.text}
          </dd>
        </div>
      ))}
    </dl>
  );
}
