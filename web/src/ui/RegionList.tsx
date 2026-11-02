import type { RefObject } from "preact";
import { cx } from "../lib/cx.ts";
import { stepIndex } from "../lib/place.ts";
import type { Level, Texture } from "./region.ts";
import { regionClass } from "./region.ts";
import styles from "./regionlist.module.css";
import { RoleChip } from "./RoleChip.tsx";
import type { RoleChipProps } from "./RoleChip.tsx";
import { StatusLine, StatusMark } from "./StatusMark.tsx";
import { Swatch } from "./Swatch.tsx";

/** The list's summary header. */
export interface RegionSummary {
  /** "8 MiB flash". */
  title: string;
  status: { level: Level; text: string };
  /** Lines under the status line; a `muted` one has no icon. */
  lines: readonly { level: Level | "muted"; text: string }[];
}

/** A row of the region list. */
export interface RegionRow {
  id: string;
  code: string;
  colour: string;
  texture: Texture;
  title: string;
  subtitle: string;
  /** A region missing from the board's table: dimmed, its title struck through. */
  missing: boolean;
  size: string;
  roles: readonly RoleChipProps[];
  status: { level: Level; word: string; reason: string };
}

/**
 * The region list: the summary header, then a row per region and gap in
 * flash order. Up and Down move the focus between rows.
 */
export function RegionList({
  label,
  summary,
  rows,
  selected,
  onSelect,
  listRef,
}: {
  label: string;
  summary: RegionSummary;
  rows: readonly RegionRow[];
  selected: string | undefined;
  onSelect: (id: string, keyboard: boolean) => void;
  listRef: RefObject<HTMLUListElement | null>;
}) {
  return (
    <>
      <div class={styles["summary"]}>
        <p class={styles["title"]}>{summary.title}</p>
        <StatusLine level={summary.status.level}>{summary.status.text}</StatusLine>
        {summary.lines.map((line) => (
          <StatusLine key={line.text} level={line.level}>
            {line.text}
          </StatusLine>
        ))}
      </div>
      <ul
        ref={listRef}
        class={styles["list"]}
        aria-label={label}
        onKeyDown={(event) => {
          const target = event.target;
          if (!(target instanceof HTMLElement) || !target.matches("[data-row]")) return;
          const all = Array.from(event.currentTarget.querySelectorAll<HTMLElement>("[data-row]"));
          const next = stepIndex(all.indexOf(target), all.length, event.key);
          if (next === undefined) return;
          event.preventDefault();
          all[next]?.focus();
        }}
      >
        {rows.map((row) => (
          <li key={row.id}>
            <button
              type="button"
              class={cx(styles["row"], row.missing && styles["missing"], regionClass(row.colour))}
              data-row=""
              data-region={row.id}
              aria-current={row.id === selected ? "true" : undefined}
              onClick={(event) => {
                onSelect(row.id, event.detail === 0);
              }}
            >
              <span class={styles["swatch"]}>
                <Swatch code={row.code} colour={row.colour} texture={row.texture} />
              </span>
              <span class={styles["name"]}>
                <b>{row.title}</b>
                <span class={styles["subtitle"]}>{row.subtitle}</span>
              </span>
              <span class={styles["status"]}>
                <StatusMark level={row.status.level} word={row.status.word} reason={row.status.reason} />
              </span>
              <span class={styles["facts"]}>
                <span>{row.size}</span>
                {row.roles.map((role) => (
                  <RoleChip key={role.role} {...role} />
                ))}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </>
  );
}
