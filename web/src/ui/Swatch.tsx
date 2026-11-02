import { cx } from "../lib/cx.ts";
import type { Texture } from "./region.ts";
import { regionClass } from "./region.ts";
import styles from "./swatch.module.css";

/**
 * A region's swatch: its code on its tint, in its texture. Decorative: the
 * region's title beside it names it.
 */
export function Swatch({
  code,
  colour,
  texture,
  size = "md",
}: {
  /** The code, or an en dash for a gap. */
  code: string;
  colour: string;
  texture: Texture;
  /** 34 px in a row, 46 px in the inspector's header, 22 px in the tray. */
  size?: "sm" | "md" | "lg";
}) {
  return (
    <span class={cx(styles["swatch"], styles[size], styles[texture], regionClass(colour))} aria-hidden="true">
      {code}
    </span>
  );
}
