import { cx } from "../lib/cx.ts";
import styles from "./region.module.css";

/** How a region's block, swatch and ruler are drawn. */
export type Texture = "public" | "sensitive" | "reserved" | "gap" | "missing" | "extra";

/** How far a region is from what the firmware expects. */
export type Level = "ok" | "warn" | "bad";

/**
 * The classes that set a region's colour variables from its colour token,
 * such as `--r-a0`; an unknown token takes the reserved region's colour.
 */
export function regionClass(colour: string): string {
  return cx(styles["region"], styles[colour.replace(/^--/, "")] ?? styles["r-rs"]);
}
