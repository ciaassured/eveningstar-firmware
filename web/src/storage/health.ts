import { formatBytes } from "../lib/format.ts";
import type { StatusLevel } from "../ui/StatusIcon.tsx";
import { faultLabel, limitFromDevelopmentFlash, limitText, tierLabel } from "./labels.ts";
import type { InternalFlash, TableEntry } from "./types.ts";
import type { RegionInfo } from "./types.ts";

/** A finding about the flash that needs the person's attention. */
export interface Notice {
  key: string;
  level: StatusLevel;
  title: string;
  lines: string[];
}

/** What the person should know about the flash, most serious first. */
export function healthNotices(flash: InternalFlash): Notice[] {
  const notices: Notice[] = [];
  const limit = flash.limit === null ? [] : [limitText(flash.limit)];
  switch (flash.tier) {
    case "full":
      break;
    case "data":
      notices.push({
        key: "tier",
        level: "caution",
        title: "Updates are unavailable",
        lines: [...limit, "The configuration is saved, and the firmware can't install or roll back updates."],
      });
      break;
    case "untrusted":
      notices.push({
        key: "tier",
        level: "serious",
        title: "Running without saved configuration",
        lines: [
          ...limit,
          "The firmware reads only the boot chain, and runs on a default configuration it doesn't save.",
          ...(limitFromDevelopmentFlash(flash.limit)
            ? [
                "This is likely a board flashed with cargo run, whose bootloader and partition table aren't the board's. Flashing the board's layout with flash-all brings it back to the full tier.",
              ]
            : []),
        ],
      });
      break;
    default:
      notices.push({ key: "tier", level: "off", title: tierLabel(flash.tier).label, lines: limit });
  }
  for (const app of flash.apps) {
    if (app.image.status !== "invalid") continue;
    const where = app.slot === null ? app.region : `${app.region} (${app.slot})`;
    notices.push({
      key: `invalid-${app.region}`,
      level: "serious",
      title: `The image in ${where} doesn't verify`,
      lines: [
        ...(app.image.error === null || app.image.error === undefined ? [] : [`${faultLabel(app.image.error)}.`]),
        "This can be the first sign of failing flash, or of a write that didn't finish.",
      ],
    });
  }
  if (flash.boot?.needs_confirmation === true) {
    notices.push({
      key: "confirm",
      level: "caution",
      title: "The running firmware isn't confirmed",
      lines: ["The bootloader rolls it back at the next reboot unless it is confirmed, which this firmware can't do yet."],
    });
  }
  if (flash.checks.status === "running") {
    notices.push({
      key: "checks",
      level: "standby",
      title: "Checking the firmware images",
      lines: ["Each image is verified and hashed in the background. This page refreshes until the checks finish."],
    });
  }
  const rank: Record<StatusLevel, number> = { critical: 0, serious: 1, caution: 2, standby: 3, normal: 4, off: 5 };
  return notices.sort((a, b) => rank[a.level] - rank[b.level]);
}

/** The flash's state in one line: its tier, size, and what that allows. */
export function flashSummary(flash: InternalFlash): string[] {
  const size = flash.capacity > 0 ? formatBytes(flash.capacity) : `${formatBytes(flash.layout_size)} layout, chip size unknown`;
  return [
    tierLabel(flash.tier).label,
    size,
    flash.persistent_config ? "configuration saved" : "configuration not saved",
    flash.updates ? "updates ready" : "no updates",
  ];
}

/** The fields of a partition entry, as the layout has them or as the board's table holds them. */
export interface EntryFields {
  label: string;
  type: number;
  subtype: number;
  offset: number;
  size: number;
  flags: number;
}

export type FieldName = keyof EntryFields;
const FIELDS: readonly FieldName[] = ["label", "type", "subtype", "offset", "size", "flags"];

/** The layout's entry and the board table's entry in its place, and which fields differ. */
export interface EntryComparison {
  index: number;
  expected?: EntryFields;
  found?: EntryFields;
  differs: FieldName[];
}

/** The layout's partitions, as entries of a table. */
export function layoutEntries(regions: readonly RegionInfo[]): EntryFields[] {
  return regions.flatMap((info) =>
    info.partition === null || info.partition === undefined
      ? []
      : [
          {
            label: info.label ?? "",
            type: info.partition.type,
            subtype: info.partition.subtype,
            offset: info.offset,
            size: info.size,
            flags: info.partition.flags,
          },
        ],
  );
}

/**
 * The board's table against the layout, entry by entry in order, as the
 * firmware compares them. Entries past the layout's are extra.
 */
export function compareEntries(expected: readonly EntryFields[], found: readonly TableEntry[]): EntryComparison[] {
  const rows: EntryComparison[] = [];
  for (let index = 0; index < Math.max(expected.length, found.length); index += 1) {
    const want = expected[index];
    const entry = found[index];
    const have: EntryFields | undefined =
      entry === undefined
        ? undefined
        : { label: entry.label, type: entry.type, subtype: entry.subtype, offset: entry.offset, size: entry.size, flags: entry.flags };
    const row: EntryComparison = {
      index,
      differs: want === undefined || have === undefined ? [] : FIELDS.filter((field) => want[field] !== have[field]),
    };
    if (want !== undefined) row.expected = want;
    if (have !== undefined) row.found = have;
    rows.push(row);
  }
  return rows;
}
