import { useRef } from "preact/hooks";
import type { EntryAction } from "../downloads/service.ts";
import { ACTION_LABELS, entryViews, traySummary } from "../downloads/view.ts";
import { DownloadList } from "../ui/DownloadList.tsx";
import type { DownloadItem } from "../ui/DownloadList.tsx";
import { Progress } from "../ui/Progress.tsx";
import { useServices } from "./context.ts";
import styles from "./shell.module.css";

/** Every download, as the tray and the Storage page list them. */
export function Downloads() {
  const { downloads, now, clipboard } = useServices();
  const items: DownloadItem[] = entryViews(downloads.entries.value, now.value, downloads.verifies).map((view) => ({
    ...view,
    actions: view.actions.map((key) => ({ key, label: ACTION_LABELS[key] })),
  }));
  return (
    <DownloadList
      items={items}
      copy={clipboard}
      onAction={(id, action) => {
        downloads.act(id, action as EntryAction);
      }}
    />
  );
}

/**
 * The header's download tray: the running download's progress and how many
 * wait, opening the list. It shows only while there are downloads.
 */
export function DownloadTray() {
  const { downloads, now } = useServices();
  const ref = useRef<HTMLDetailsElement>(null);
  const entries = downloads.entries.value;
  if (entries.length === 0) return null;
  const active = downloads.active.value;
  const state = active?.state;
  const progress =
    state?.kind === "downloading" && state.total !== undefined && state.total > 0 ? state.received / state.total : undefined;
  const summary = traySummary(entries, now.value);
  const finished = entries.filter((entry) => ["done", "cancelled"].includes(entry.state.kind));

  return (
    <details
      ref={ref}
      class={styles["tray"]}
      onKeyDown={(event) => {
        if (event.key === "Escape" && ref.current !== null && ref.current.open) {
          // The page's own Escape, such as closing the phone's inspector, waits for the next press.
          event.preventDefault();
          ref.current.open = false;
          ref.current.querySelector("summary")?.focus();
        }
      }}
    >
      <summary class={styles["traySummary"]}>
        <svg class={styles["trayIcon"]} viewBox="0 0 12 12" aria-hidden="true" focusable="false">
          <path d="M6 1v6.5M3 5l3 3 3-3M1.5 10.5h9" fill="none" stroke="currentColor" stroke-width="1.5" />
        </svg>
        <span>
          <span class="visually-hidden">Downloads: </span>
          {summary}
        </span>
        {active === undefined ? null : (
          <span class={styles["trayProgress"]}>
            <Progress value={progress} label="Download progress" tone="header" />
          </span>
        )}
      </summary>
      <div class={styles["trayPanel"]}>
        <div class={styles["trayHead"]}>
          <h2 class={styles["trayTitle"]}>Downloads</h2>
          {finished.length > 0 ? (
            <button
              type="button"
              class={styles["trayClear"]}
              onClick={() => {
                for (const entry of finished) downloads.act(entry.id, "dismiss");
              }}
            >
              Clear finished
            </button>
          ) : null}
        </div>
        <Downloads />
      </div>
    </details>
  );
}
