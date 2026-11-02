import { cx } from "../lib/cx.ts";
import styles from "./progress.module.css";

/**
 * A bar for how much of something is done or filled. `label` names it for
 * assistive technology; the figures go beside it in words.
 */
export function Progress({
  value,
  label,
  tone = "accent",
}: {
  /** The share done, in [0, 1]; `undefined` while it isn't known. */
  value: number | undefined;
  label: string;
  /** `accent` for work in progress, `header` in the dark header. */
  tone?: "accent" | "header";
}) {
  return value === undefined ? (
    <progress class={cx(styles["progress"], styles[tone])} aria-label={label} />
  ) : (
    <progress class={cx(styles["progress"], styles[tone])} aria-label={label} max={1} value={Math.max(0, Math.min(1, value))} />
  );
}
