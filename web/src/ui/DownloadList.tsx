import { CopyDigest } from "./Digest.tsx";
import { Progress } from "./Progress.tsx";
import { StatusIcon } from "./StatusIcon.tsx";
import type { StatusLevel } from "./StatusIcon.tsx";
import styles from "./downloads.module.css";

/** A download as the list shows it, its state already in words. */
export interface DownloadItem {
  id: number;
  label: string;
  filename?: string | undefined;
  level: StatusLevel;
  status: string;
  detail?: string | undefined;
  progress?: number | undefined;
  expected?: { sha256: string; command: string } | undefined;
  actions: readonly { key: string; label: string }[];
}

/** The downloads, in queue order, each with its state and what can be done with it. */
export function DownloadList({
  items,
  onAction,
  copy,
}: {
  items: readonly DownloadItem[];
  onAction: (id: number, action: string) => void;
  copy: ((text: string) => Promise<void>) | undefined;
}) {
  if (items.length === 0) return <p class={styles["empty"]}>No downloads.</p>;
  return (
    <ol class={styles["list"]}>
      {items.map((item) => (
        <li key={item.id} class={styles["item"]}>
          <div class={styles["head"]}>
            <span class={styles["label"]}>{item.label}</span>
            {item.filename === undefined ? null : <span class={styles["file"]}>{item.filename}</span>}
          </div>
          <p class={styles["status"]}>
            <StatusIcon level={item.level} />
            <span>{item.status}</span>
          </p>
          {item.progress === undefined ? null : <Progress value={item.progress} label={`${item.label} progress`} />}
          {item.detail === undefined ? null : <p class={styles["detail"]}>{item.detail}</p>}
          {item.expected === undefined ? null : (
            <div class={styles["expected"]}>
              <span class={styles["detail"]}>Not checked here, as the page isn't served over HTTPS. Expected SHA-256:</span>
              <CopyDigest value={item.expected.sha256} label="expected SHA-256" copy={copy} />
              <code class={styles["command"]}>{item.expected.command}</code>
            </div>
          )}
          {item.actions.length > 0 ? (
            <div class={styles["actions"]}>
              {item.actions.map((action) => (
                <button
                  key={action.key}
                  type="button"
                  class={styles["action"]}
                  aria-label={`${action.label}: ${item.label}`}
                  onClick={() => {
                    onAction(item.id, action.key);
                  }}
                >
                  {action.label}
                </button>
              ))}
            </div>
          ) : null}
        </li>
      ))}
    </ol>
  );
}
