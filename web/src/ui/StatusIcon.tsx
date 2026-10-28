import { cx } from "../lib/cx.ts";
import styles from "./status.module.css";

/** The six levels of status, each a shape and a colour. */
export type StatusLevel = "off" | "standby" | "normal" | "caution" | "serious" | "critical";

export const STATUS_LABELS: Record<StatusLevel, string> = {
  off: "Off",
  standby: "Standby",
  normal: "Normal",
  caution: "Caution",
  serious: "Serious",
  critical: "Critical",
};

function Shape({ level }: { level: StatusLevel }) {
  switch (level) {
    case "off":
      return <circle cx="6" cy="6" r="4.25" fill="none" stroke="currentColor" stroke-width="1.5" />;
    case "standby":
      return (
        <>
          <circle cx="6" cy="6" r="4.25" fill="none" stroke="currentColor" stroke-width="1.5" />
          <path d="M6 1.75a4.25 4.25 0 0 1 0 8.5Z" fill="currentColor" />
        </>
      );
    case "normal":
      return <circle cx="6" cy="6" r="5" fill="currentColor" />;
    case "caution":
      return <path d="M6 .75 11.5 10.75h-11Z" fill="currentColor" />;
    case "serious":
      return <path d="M6 .5 11.5 6 6 11.5.5 6Z" fill="currentColor" />;
    case "critical":
      return <path d="M3.75.5h4.5l3.25 3.25v4.5L8.25 11.5h-4.5L.5 8.25v-4.5Z" fill="currentColor" />;
  }
}

/** A status level's shape, in its colour. It carries no meaning of its own: a word always goes beside it. */
export function StatusIcon({ level, class: className }: { level: StatusLevel; class?: string }) {
  return (
    <svg
      class={cx(styles["icon"], styles[level], className)}
      viewBox="0 0 12 12"
      aria-hidden="true"
      focusable="false"
    >
      <Shape level={level} />
    </svg>
  );
}

/** A status level's shape and its word. */
export function Status({ level, label }: { level: StatusLevel; label?: string }) {
  return (
    <span class={styles["status"]}>
      <StatusIcon level={level} />
      <span>{label ?? STATUS_LABELS[level]}</span>
    </span>
  );
}
