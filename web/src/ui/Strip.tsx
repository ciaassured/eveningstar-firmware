import { cx } from "../lib/cx.ts";
import { Icon } from "./Icon.tsx";
import { HelpButton } from "./Popover.tsx";
import type { Texture } from "./region.ts";
import { regionClass } from "./region.ts";
import styles from "./strip.module.css";
import { percent, useVars } from "./vars.ts";

/** A block of the flash strip: a region or a gap. */
export interface StripBlock {
  id: string;
  /** The code at its top left, such as `A0`; empty for a gap. */
  code: string;
  /** The name after the code, shown in a block 84 px wide or more. */
  title: string;
  colour: string;
  texture: Texture;
  /** Whether it carries the lock: a sensitive region. */
  lock: boolean;
  /** Whether it carries the warning, for every status other than ok. */
  warn: boolean;
  /** Its `flex-grow`. */
  grow: number;
  /** The used share of the region, for the usage bar. */
  usage?: number | undefined;
  /** The part the usage bar stands for, for the emphasis of parts. */
  usagePart?: string | undefined;
  /** The accessible name. */
  name: string;
}

/** What the readout line says of a block. */
export interface StripReadout {
  id: string;
  title: string;
  subtitle: string;
  start: string;
  end: string;
  size: string;
}

function Block({
  block,
  selected,
  onSelect,
}: {
  block: StripBlock;
  selected: boolean;
  onSelect: (id: string, keyboard: boolean) => void;
}) {
  const ref = useVars<HTMLButtonElement>({ "--w": block.grow.toFixed(3) });
  const used = useVars<HTMLSpanElement>({ "--u": percent(block.usage ?? 0) });
  const icons = (block.lock ? 1 : 0) + (block.warn ? 1 : 0);
  return (
    <button
      ref={ref}
      type="button"
      class={cx(styles["block"], styles[block.texture], regionClass(block.colour))}
      data-region={block.id}
      data-icons={icons}
      data-warn={block.warn ? "" : undefined}
      aria-label={block.name}
      aria-current={selected ? "true" : undefined}
      onClick={(event) => {
        onSelect(block.id, event.detail === 0);
      }}
    >
      {block.usage === undefined ? null : (
        <span class={styles["track"]}>
          <span ref={used} class={styles["used"]} data-part={block.usagePart} />
        </span>
      )}
      <span class={styles["code"]}>
        {block.code}
        <span class={styles["name"]}>{block.title}</span>
      </span>
      {icons === 0 ? null : (
        <span class={styles["icons"]}>
          {block.lock ? (
            <span class={styles["lock"]}>
              <Icon name="lock" size={12} />
            </span>
          ) : null}
          {block.warn ? (
            <span class={styles["warn"]}>
              <Icon name="warn" size={12} />
            </span>
          ) : null}
        </span>
      )}
    </button>
  );
}

/**
 * The flash strip: one block per region and gap in flash order, each wide
 * enough to see and otherwise in proportion to its size, and under it the
 * readout line naming the region pointed at, else the selected one.
 */
export function Strip({
  label,
  blocks,
  selected,
  onSelect,
  readouts,
  ends,
  help,
}: {
  label: string;
  blocks: readonly StripBlock[];
  selected: string | undefined;
  onSelect: (id: string, keyboard: boolean) => void;
  readouts: readonly StripReadout[];
  /** The strip's first offset, and its end and size. */
  ends: { start: string; end: string };
  help: { label: string; title: string; text: string };
}) {
  return (
    <div class={styles["band"]}>
      <div class={styles["strip"]} role="group" aria-label={label}>
        {blocks.map((block) => (
          <Block key={block.id} block={block} selected={block.id === selected} onSelect={onSelect} />
        ))}
      </div>
      <div class={styles["axis"]}>
        <span class={styles["end"]}>{ends.start}</span>
        <span class={styles["readout"]}>
          {readouts.map((readout) => (
            <span key={readout.id} data-readout={readout.id} data-selected={readout.id === selected ? "" : undefined}>
              <b>{readout.title}</b> {readout.subtitle}
              <span class={styles["offsets"]}>
                : <span class={styles["mono"]}>{readout.start}</span> to <span class={styles["mono"]}>{readout.end}</span>
              </span>
              , {readout.size}
            </span>
          ))}
        </span>
        <HelpButton label={help.label} title={help.title} text={help.text} />
        <span class={styles["end"]}>{ends.end}</span>
      </div>
    </div>
  );
}
