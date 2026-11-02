import { cx } from "../lib/cx.ts";
import styles from "./placement.module.css";
import { percent, useVars } from "./vars.ts";

/** A span on a placement bar, as shares of the bar, with the caption under it. */
interface PlacedSpan {
  start: number;
  width: number;
  caption: string;
}

function Bar({ span, board }: { span: PlacedSpan; board: boolean }) {
  const ref = useVars<HTMLSpanElement>({ "--l": percent(span.start), "--w": percent(span.width) });
  return (
    <>
      <div class={cx(styles["bar"], board && styles["board"])} aria-hidden="true">
        <span ref={ref} />
      </div>
      <span>{span.caption}</span>
    </>
  );
}

/** Where a region is in the expected layout and on the board, to scale: the caption, then a bar and its caption for each. */
export function Placement({ caption, expected, board }: { caption: string; expected: PlacedSpan; board: PlacedSpan }) {
  return (
    <div class={styles["placement"]}>
      <span class={styles["caption"]}>{caption}</span>
      <Bar span={expected} board={false} />
      <Bar span={board} board />
    </div>
  );
}
