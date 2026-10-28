import styles from "./brand.module.css";
import {
  MARK_DOTS,
  MARK_RINGS,
  MARK_STAR_DAY,
  MARK_STAR_NIGHT,
  MARK_STAR_TRANSFORM,
  MARK_VIEW_BOX,
  WORDMARK,
  WORDMARK_TRANSFORM,
  WORDMARK_VIEW_BOX,
} from "./brand-paths.ts";

/**
 * The mark: a four-point star split by the day-night curve, its day half in the
 * day colour and its night half, ring and dots in the night colour of the
 * surface it sits on (`--mark-day`, `--mark-night`).
 */
export function Mark() {
  return (
    <svg class={styles["mark"]} viewBox={MARK_VIEW_BOX} aria-hidden="true" focusable="false">
      <g transform={MARK_STAR_TRANSFORM}>
        <path class={styles["day"]} d={MARK_STAR_DAY} />
        <path class={styles["night"]} d={MARK_STAR_NIGHT} />
      </g>
      <g class={styles["night"]}>
        <path d={MARK_RINGS} />
        {MARK_DOTS.map(([cx, cy, r]) => (
          <circle key={cx} cx={cx} cy={cy} r={r} />
        ))}
      </g>
    </svg>
  );
}

/** The name, in the colour of the text around it. */
export function Wordmark() {
  return (
    <svg class={styles["wordmark"]} viewBox={WORDMARK_VIEW_BOX} role="img" aria-label="Eveningstar">
      <path d={WORDMARK} transform={WORDMARK_TRANSFORM} />
    </svg>
  );
}
