import styles from "./refreshbutton.module.css";

/** The header's Refresh button: a pill with the reload icon, which turns while the reload runs. */
export function RefreshButton({
  label,
  busyLabel,
  refreshing,
  onClick,
}: {
  label: string;
  busyLabel: string;
  refreshing: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      class={styles["button"]}
      disabled={refreshing}
      aria-busy={refreshing ? "true" : undefined}
      aria-label={refreshing ? busyLabel : label}
      onClick={onClick}
    >
      <svg class={styles["icon"]} data-turning={refreshing ? "true" : undefined} viewBox="0 0 16 16" aria-hidden="true">
        <path d="M13.2 8A5.2 5.2 0 1 1 11.7 4.3" />
        <path d="M13.4 2.2v3.3h-3.3" />
      </svg>
      <span class={styles["label"]}>{refreshing ? busyLabel : label}</span>
    </button>
  );
}
