import type { SpanText } from "../lib/format.ts";
import { cx } from "../lib/cx.ts";
import { HelpButton } from "./Popover.tsx";
import { regionClass } from "./region.ts";
import type { Fill } from "./Ruler.tsx";
import styles from "./spantable.module.css";

/** A row of a span table. */
export interface SpanRow {
  /** The key for the emphasis of parts, shared with the part on the ruler and the strip. */
  part: string;
  label: string;
  /** The key square: the region's tint, a part's fill, or another region's colour. */
  square: "region" | Fill | "other";
  /** For a row of another region, its colour token. */
  colour?: string | undefined;
  /** For a row of another region, its key, so that pointing at the row emphasises it. */
  region?: string | undefined;
  span: SpanText;
}

/** What the header of a span table says. */
export interface SpanHeaders {
  what: string;
  start: string;
  end: string;
  size: string;
  /** The help of "End, exclusive". */
  help: { label: string; title: string; text: string };
}

/** An offset: the hex, and the decimal beside it. */
function Offset({ hex, decimal }: { hex: string; decimal: string }) {
  return (
    <>
      <span class={styles["hex"]}>{hex}</span>
      <span class={styles["decimal"]}>{decimal}</span>
    </>
  );
}

/**
 * A table of ranges: the region, then its parts, each with its start, its
 * end (exclusive) and its size, in hex and in decimal.
 */
export function SpanTable({ label, headers, rows }: { label: string; headers: SpanHeaders; rows: readonly SpanRow[] }) {
  return (
    <div class={styles["table"]} role="table" aria-label={label}>
      <div class={styles["head"]} role="row">
        <span class={styles["cell"]} role="columnheader">
          {headers.what}
        </span>
        <span class={styles["cell"]} role="columnheader">
          {headers.start}
        </span>
        <span class={styles["cell"]} role="columnheader">
          {headers.end}
          <HelpButton {...headers.help} />
        </span>
        <span class={styles["cell"]} role="columnheader">
          {headers.size}
        </span>
      </div>
      {rows.map((row) => (
        <div key={row.part} class={styles["row"]} role="row" data-part={row.part} data-region={row.region}>
          <span class={styles["cell"]} role="cell">
            <span class={cx(styles["key"], styles[row.square], row.colour === undefined ? undefined : regionClass(row.colour))} />
            {row.label}
          </span>
          <span class={styles["cell"]} role="cell">
            <Offset {...row.span.start} />
          </span>
          <span class={styles["cell"]} role="cell">
            <Offset {...row.span.end} />
          </span>
          <span class={styles["cell"]} role="cell">
            <span class={styles["hex"]}>{row.span.size.hex}</span>
            <span class={styles["decimal"]}>{row.span.size.text}</span>
          </span>
        </div>
      ))}
    </div>
  );
}
