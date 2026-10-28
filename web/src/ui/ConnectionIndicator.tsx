import { cx } from "../lib/cx.ts";
import styles from "./connection.module.css";

export type ConnectionLevel = "online" | "degraded" | "offline";

const WORDS: Record<ConnectionLevel, string> = {
  online: "Online",
  degraded: "Retrying",
  offline: "Offline",
};

/**
 * A dot and a word for whether the device answers. Changes are announced to
 * assistive technology.
 */
export function ConnectionIndicator({ state }: { state: ConnectionLevel }) {
  return (
    <span class={styles["indicator"]} role="status" aria-live="polite">
      <span class={cx(styles["dot"], styles[state])} aria-hidden="true" />
      <span>{WORDS[state]}</span>
    </span>
  );
}
