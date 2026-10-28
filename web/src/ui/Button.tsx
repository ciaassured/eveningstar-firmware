import type { ComponentChildren } from "preact";
import { cx } from "../lib/cx.ts";
import styles from "./button.module.css";

/**
 * A button. When it is disabled for a reason, the reason is shown beside it,
 * so the person isn't left to guess.
 */
export function Button({
  variant = "secondary",
  disabled = false,
  reason,
  onClick,
  children,
}: {
  variant?: "primary" | "secondary";
  disabled?: boolean;
  reason?: string | undefined;
  onClick?: () => void;
  children: ComponentChildren;
}) {
  return (
    <span class={styles["wrap"]}>
      <button
        type="button"
        class={cx(styles["button"], styles[variant])}
        disabled={disabled}
        onClick={onClick}
      >
        {children}
      </button>
      {disabled && reason !== undefined ? <span class={styles["reason"]}>{reason}</span> : null}
    </span>
  );
}

/** Buttons side by side. */
export function Actions({ children }: { children: ComponentChildren }) {
  return <div class={styles["actions"]}>{children}</div>;
}
