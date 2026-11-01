import type { ComponentChildren } from "preact";
import { cx } from "../lib/cx.ts";
import styles from "./facts.module.css";

/** Labelled values in two columns, or stacked when narrow. */
export function Facts({ children, compact }: { children: ComponentChildren; compact?: boolean }) {
  return <dl class={cx(styles["facts"], compact && styles["compact"])}>{children}</dl>;
}

export function Fact({ label, children, mono }: { label: string; children: ComponentChildren; mono?: boolean }) {
  return (
    <div class={styles["fact"]}>
      <dt class={styles["label"]}>{label}</dt>
      <dd class={cx(styles["value"], mono && styles["mono"])}>{children}</dd>
    </div>
  );
}

/** A short word marking what something is to the device, such as Running. */
export function Badge({ children, accent }: { children: ComponentChildren; accent?: boolean }) {
  return <span class={cx(styles["badge"], accent && styles["accent"])}>{children}</span>;
}

export function Badges({ children }: { children: ComponentChildren }) {
  return <div class={styles["badges"]}>{children}</div>;
}

/** A padlock and why something can't be had; the reason is read out, and shown beside it when `visible`. */
export function LockNote({ reason, visible }: { reason: string; visible?: boolean }) {
  return (
    <span class={styles["lock"]} title={reason}>
      <svg class={styles["lockIcon"]} viewBox="0 0 12 14" aria-hidden="true" focusable="false">
        <path d="M3 6V4a3 3 0 0 1 6 0v2" fill="none" stroke="currentColor" stroke-width="1.5" />
        <rect x="1.5" y="6" width="9" height="7" rx="1.3" fill="currentColor" />
      </svg>
      <span class={visible === true ? styles["reason"] : "visually-hidden"} data-lock-reason="">
        {reason}
      </span>
    </span>
  );
}

/** A button styled as a plain text action, for actions inside tables and lists. */
export function TextButton({
  children,
  onClick,
  label,
}: {
  children: ComponentChildren;
  onClick: () => void;
  /** The accessible name, when the visible text is too short to say what it does. */
  label?: string | undefined;
}) {
  return (
    <button type="button" class={styles["textButton"]} onClick={onClick} aria-label={label}>
      {children}
    </button>
  );
}
