import type { ComponentChildren } from "preact";
import { cx } from "../lib/cx.ts";
import { Status } from "./StatusIcon.tsx";
import type { StatusLevel } from "./StatusIcon.tsx";
import styles from "./regiontable.module.css";

/** A row of the region table, its values already in words. */
export interface RegionRow {
  key: string;
  /** The row's element id, so the flash map can bring it into view. */
  id: string;
  name: string;
  /** Whether it is unallocated space rather than a region. */
  gap: boolean;
  /** Whether it is the running app, which carries the selection bar. */
  current: boolean;
  partition: string;
  offset: string;
  offsetTitle: string;
  size: string;
  sizeTitle: string;
  contents: string;
  layout?: { level: StatusLevel; label: string } | undefined;
  /** What expanding the row shows; a row without details doesn't expand. */
  details?: ComponentChildren;
  /** The download button, or the lock with its reason. */
  action?: ComponentChildren;
}

const HEADINGS = ["Region", "Partition", "Offset", "Size", "Contents", "Layout", "Download"] as const;

/**
 * The regions of the flash in a table, each row expanding to its details. In
 * a narrow container each row becomes a card.
 */
export function RegionTable({
  caption,
  rows,
  expanded,
  onToggle,
}: {
  caption: string;
  rows: readonly RegionRow[];
  expanded: ReadonlySet<string>;
  onToggle: (key: string) => void;
}) {
  return (
    <div class={styles["wrap"]}>
      <table class={styles["table"]}>
        <caption class="visually-hidden">{caption}</caption>
        <thead>
          <tr>
            {HEADINGS.map((heading) => (
              <th key={heading} scope="col">
                {heading}
              </th>
            ))}
          </tr>
        </thead>
        {rows.map((row) => {
          const open = expanded.has(row.key);
          const detailsId = `${row.id}-details`;
          return (
            <tbody key={row.key} id={row.id} tabIndex={-1} class={cx(styles["row"], row.current && styles["current"], row.gap && styles["gap"])}>
              <tr>
                <th scope="row" data-label={HEADINGS[0]}>
                  {row.details === undefined ? (
                    <span class={styles["name"]}>{row.name}</span>
                  ) : (
                    <button
                      type="button"
                      class={styles["toggle"]}
                      aria-expanded={open}
                      aria-controls={detailsId}
                      onClick={() => {
                        onToggle(row.key);
                      }}
                    >
                      <svg class={styles["chevron"]} viewBox="0 0 10 10" aria-hidden="true" focusable="false">
                        <path d="M3 1.5 6.5 5 3 8.5" fill="none" stroke="currentColor" stroke-width="1.5" />
                      </svg>
                      <span class={styles["name"]}>{row.name}</span>
                    </button>
                  )}
                </th>
                <td data-label={HEADINGS[1]} class={styles["mono"]}>
                  {row.partition}
                </td>
                <td data-label={HEADINGS[2]} class={styles["mono"]} title={row.offsetTitle}>
                  {row.offset}
                </td>
                <td data-label={HEADINGS[3]} title={row.sizeTitle}>
                  {row.size}
                </td>
                <td data-label={HEADINGS[4]}>{row.contents}</td>
                <td data-label={HEADINGS[5]}>
                  {row.layout === undefined ? "–" : <Status level={row.layout.level} label={row.layout.label} />}
                </td>
                <td data-label={HEADINGS[6]} class={styles["action"]}>
                  {row.action}
                </td>
              </tr>
              {open && row.details !== undefined ? (
                <tr class={styles["details"]} id={detailsId}>
                  <td colSpan={HEADINGS.length}>{row.details}</td>
                </tr>
              ) : null}
            </tbody>
          );
        })}
      </table>
    </div>
  );
}
