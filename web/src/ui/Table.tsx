import type { ComponentChildren } from "preact";
import { cx } from "../lib/cx.ts";
import styles from "./table.module.css";

/** A row of a small table: its cells, bold (`strong`) or in the warning colour (`warn`). */
interface TableRow {
  key: string;
  cells: readonly ComponentChildren[];
  tone?: "strong" | "warn" | undefined;
}

/**
 * A small table of facts. `records` (the otadata's records) keeps each cell on
 * one line and scrolls sideways in its own container when narrow; `fields`
 * (a comparison) heads each row with its first cell.
 */
export function Table({ kind, columns, rows }: { kind: "records" | "fields"; columns: readonly string[]; rows: readonly TableRow[] }) {
  const table = (
    <table class={cx(styles["table"], styles[kind])}>
      <thead>
        <tr>
          {columns.map((column) => (
            <th key={column} scope="col">
              {column}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.key} class={row.tone === undefined ? undefined : styles[row.tone]}>
            {row.cells.map((cell, i) =>
              i === 0 && kind === "fields" ? (
                <th key={i} scope="row">
                  {cell}
                </th>
              ) : (
                <td key={i}>{cell}</td>
              ),
            )}
          </tr>
        ))}
      </tbody>
    </table>
  );
  return kind === "records" ? <div class={styles["scroll"]}>{table}</div> : table;
}
