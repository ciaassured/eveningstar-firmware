import { useId } from "preact/hooks";
import { cx } from "../lib/cx.ts";
import styles from "./flashmap.module.css";

/** A part of the flash as the map draws it; positions are shares of the bar. */
export interface FlashMapSegment {
  key: string;
  name: string;
  kind: "public" | "sensitive" | "reserved" | "gap";
  start: number;
  width: number;
  /** Whether it carries the selection bar. */
  current: boolean;
  /** The share of it an image fills. */
  fill?: number | undefined;
  /** Whether it is wide enough to carry its name; narrower ones are named in the legend. */
  labelled: boolean;
}

const BAR = 36;
const STRIP = 6;
const pct = (share: number): string => `${(share * 100).toFixed(3)}%`;

/** A padlock, centred on 0 horizontally, 12 tall, for the map's sensitive regions. */
function Lock({ y }: { y: number }) {
  return (
    <g class={styles["lock"]} transform={`translate(0 ${y})`}>
      <path d="M-3 0v-2.5a3 3 0 0 1 6 0V0" fill="none" stroke-width="1.6" />
      <rect x="-4.5" y="0" width="9" height="7" rx="1.2" />
    </g>
  );
}

/**
 * One bar for the whole flash, its segments proportional to their size. The
 * table beside it is the accessible source of truth, so the map is hidden
 * from assistive technology; a click on a segment calls `onSelect` with its key.
 */
export function FlashMap({
  segments,
  onSelect,
}: {
  segments: readonly FlashMapSegment[];
  onSelect: (key: string) => void;
}) {
  const hatch = useId();
  const narrow = segments.filter((segment) => !segment.labelled);
  return (
    <div class={styles["map"]} aria-hidden="true">
      <svg class={styles["bar"]} width="100%" height={BAR + 6} focusable="false">
        <defs>
          <pattern id={hatch} width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
            <rect class={styles["hatch"]} width="1.5" height="6" />
          </pattern>
        </defs>
        {segments.map((segment) => (
          <g
            key={segment.key}
            class={cx(styles["segment"], styles[segment.kind])}
            onClick={() => {
              onSelect(segment.key);
            }}
          >
            <title>{segment.name}</title>
            <rect
              class={styles["area"]}
              x={pct(segment.start)}
              width={pct(segment.width)}
              y="0"
              height={BAR}
              fill={segment.kind === "gap" ? `url(#${hatch})` : undefined}
            />
            {segment.fill === undefined ? null : (
              <rect
                class={styles["fill"]}
                x={pct(segment.start)}
                width={pct(segment.width * segment.fill)}
                y={BAR - STRIP}
                height={STRIP}
              />
            )}
            {segment.labelled ? (
              <svg x={pct(segment.start)} width={pct(segment.width)} y="0" height={BAR - STRIP}>
                <text class={styles["label"]} x="50%" y="19" text-anchor="middle">
                  {segment.name}
                </text>
              </svg>
            ) : null}
            {segment.kind === "sensitive" ? (
              <svg
                x={pct(segment.start)}
                width={pct(segment.width)}
                y="0"
                height={BAR - STRIP}
                viewBox={segment.labelled ? "-8 0 2000 30" : "-1000 0 2000 30"}
                preserveAspectRatio={segment.labelled ? "xMinYMid slice" : "xMidYMid slice"}
              >
                <Lock y={14} />
              </svg>
            ) : null}
            {segment.current ? (
              <rect class={styles["selected"]} x={pct(segment.start)} width={pct(segment.width)} y={BAR + 2} height="3" />
            ) : null}
          </g>
        ))}
      </svg>
      {narrow.length > 0 ? (
        <ol class={styles["legend"]}>
          {narrow.map((segment) => (
            <li key={segment.key}>
              <button
                type="button"
                tabIndex={-1}
                class={styles["legendItem"]}
                onClick={() => {
                  onSelect(segment.key);
                }}
              >
                <svg class={cx(styles["swatch"], styles[segment.kind])} viewBox="0 0 10 10" focusable="false">
                  <rect class={styles["area"]} width="10" height="10" fill={segment.kind === "gap" ? `url(#${hatch})` : undefined} />
                </svg>
                {segment.name}
              </button>
            </li>
          ))}
        </ol>
      ) : null}
    </div>
  );
}
