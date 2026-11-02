import { cx } from "../lib/cx.ts";
import type { Texture } from "./region.ts";
import styles from "./ruler.module.css";
import { percent, useVars } from "./vars.ts";

/** How a part of a region is filled: used, the alternate record, or unused and erased. */
export type Fill = "used" | "alternate" | "unused";

/** A part of the region on the ruler, to scale. */
export interface RulerPart {
  /** The part's key for the emphasis of parts, shared with its span table row and strip bar. */
  part: string;
  fill: Fill;
  /** Where it starts and how wide it is, as shares of the ruler. */
  start: number;
  width: number;
  /** The label inside it, when it is wide enough to carry one. */
  caption?: { short: string; size: string; percent?: string | undefined } | undefined;
  tooltip: string;
}

/** A boundary under the ruler. */
export interface RulerTick {
  text: string;
  /** Its place on the ruler, as a share. */
  position: number;
  row: 1 | 2;
  align: "start" | "center" | "end";
}

function Part({ part }: { part: RulerPart }) {
  const ref = useVars<HTMLDivElement>({ "--l": percent(part.start), "--w": percent(part.width) });
  return (
    <div ref={ref} class={cx(styles["part"], styles[part.fill])} data-part={part.part} title={part.tooltip}>
      {part.caption === undefined ? null : (
        <span class={styles["label"]}>
          <b>{part.caption.short}</b> {part.caption.size}
          {part.caption.percent === undefined ? null : <span class={styles["percent"]}>, {part.caption.percent}</span>}
        </span>
      )}
    </div>
  );
}

function Tick({ tick }: { tick: RulerTick }) {
  const ref = useVars<HTMLSpanElement>({ "--x": percent(tick.position), "--s": tick.position.toFixed(5) });
  return (
    <span ref={ref} class={cx(styles["tick"], tick.row === 2 && styles["second"], styles[tick.align])}>
      <span class={styles["offset"]}>{tick.text}</span>
    </span>
  );
}

/**
 * A region drawn to scale, in its texture, with its parts placed by offset
 * and size, and the offsets of their boundaries under it. A region without
 * parts carries one label over the whole ruler.
 */
export function Ruler({
  part,
  texture,
  parts,
  text,
  ticks,
}: {
  /** The whole region's key for the emphasis of parts. */
  part: string;
  texture: Texture;
  parts: readonly RulerPart[];
  text?: string | undefined;
  ticks: readonly RulerTick[];
}) {
  return (
    <>
      <div class={cx(styles["ruler"], styles[texture])} data-part={part}>
        {parts.map((item) => (
          <Part key={item.part} part={item} />
        ))}
        {text === undefined ? null : <span class={styles["label"]}>{text}</span>}
      </div>
      <div class={cx(styles["ticks"], ticks.some((tick) => tick.row === 2) && styles["two"])} aria-hidden="true">
        {ticks.map((tick) => (
          <Tick key={tick.text} tick={tick} />
        ))}
      </div>
    </>
  );
}
