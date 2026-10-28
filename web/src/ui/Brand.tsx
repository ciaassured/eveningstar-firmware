import styles from "./brand.module.css";
import {
  MARK_DOTS,
  MARK_GRADIENT,
  MARK_RINGS,
  MARK_STAR,
  MARK_STAR_TRANSFORM,
  MARK_VIEW_BOX,
  WORDMARK,
  WORDMARK_TRANSFORM,
  WORDMARK_VIEW_BOX,
} from "./brand-paths.ts";

const GRADIENT_ID = "eveningstar-mark-gradient";

/** The mark: a four-point star on the day-night curve, in the moon and sun colours. */
export function Mark() {
  const [sun, moon] = MARK_GRADIENT.stops;
  return (
    <svg class={styles["mark"]} viewBox={MARK_VIEW_BOX} aria-hidden="true" focusable="false">
      <defs>
        <radialGradient
          id={GRADIENT_ID}
          gradientUnits="userSpaceOnUse"
          cx={MARK_GRADIENT.cx}
          cy={MARK_GRADIENT.cy}
          r={MARK_GRADIENT.r}
          gradientTransform={MARK_GRADIENT.transform}
        >
          <stop class={styles["sun"]} offset={sun} />
          <stop class={styles["moon"]} offset={moon} />
        </radialGradient>
      </defs>
      <g fill={`url(#${GRADIENT_ID})`}>
        <path d={MARK_STAR} transform={MARK_STAR_TRANSFORM} />
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
