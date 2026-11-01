import { formatBytes, formatExactBytes, formatHex } from "../lib/format.ts";
import type { StatusLevel } from "../ui/StatusIcon.tsx";
import {
  bootloaderStatusLabel,
  contentsLabel,
  faultLabel,
  imageStatusLabel,
  isReadOnly,
  lockReason,
  matchLabel,
  otaStateLabel,
  roleLabel,
  subtypeName,
  typeName,
} from "./labels.ts";
import type { Labelled } from "./labels.ts";
import type { Area } from "./map.ts";
import { holdsImage } from "./plan.ts";
import type { DownloadWhat } from "./plan.ts";
import type { AppSlot, InternalFlash, Region, RegionInfo } from "./types.ts";

/** A choice of what to download from a region. */
export interface DownloadChoice {
  key: "whole" | "image" | "range";
  label: string;
  hint?: string;
}

/** A row of the region table, in words. */
export interface RegionRowView {
  key: string;
  id: string;
  name: string;
  region?: Region;
  gap: boolean;
  current: boolean;
  partition: string;
  offset: string;
  offsetTitle: string;
  size: string;
  sizeTitle: string;
  contents: string;
  layout?: Labelled;
  /** Why it can't be downloaded; `undefined` when it can, or for a gap. */
  lock?: string;
  /** What can be downloaded from it, when it can. */
  choices: DownloadChoice[];
}

/** The element id of an area's table row. */
export function rowId(key: string): string {
  return `region-${key}`;
}

function sizeTitle(size: number): string {
  return `${formatExactBytes(size)} (0x${size.toString(16)})`;
}

/** The image an app slot or the bootloader holds, with its length when known. */
export function imageLength(region: Region, flash: InternalFlash): number | undefined {
  if (region === "bootloader") return flash.bootloader.size ?? undefined;
  return flash.apps.find((app) => app.region === region)?.image.size ?? undefined;
}

/** What can be downloaded from a region the person may download. */
export function downloadChoices(info: RegionInfo, flash: InternalFlash): DownloadChoice[] {
  if (!holdsImage(info.region)) return [{ key: "whole", label: "Download", hint: formatBytes(info.size) }];
  const app = info.region !== "bootloader";
  const choices: DownloadChoice[] = [];
  const length = imageLength(info.region, flash);
  if (length !== undefined) choices.push({ key: "image", label: "Image only", hint: formatBytes(length) });
  choices.push({ key: "whole", label: app ? "Whole slot" : "Whole region", hint: formatBytes(info.size) });
  choices.push({ key: "range", label: "Custom range…" });
  return choices;
}

/** The table's row for an area. */
export function regionRow(area: Area, flash: InternalFlash): RegionRowView {
  const end = area.offset + area.size - 1;
  const base = {
    offset: formatHex(area.offset),
    offsetTitle: `${formatExactBytes(area.offset)} from the start; ends at ${formatHex(end)}`,
    size: formatBytes(area.size),
    sizeTitle: sizeTitle(area.size),
  };
  if (area.kind === "gap") {
    const key = `gap-${area.offset}`;
    return {
      ...base,
      key,
      id: rowId(key),
      name: "(unallocated)",
      gap: true,
      current: false,
      partition: "",
      contents: "",
      choices: [],
    };
  }
  const { info } = area;
  const lock = lockReason(info, flash.tier);
  const row: RegionRowView = {
    ...base,
    key: info.region,
    id: rowId(info.region),
    name: info.region,
    region: info.region,
    gap: false,
    current: info.roles.includes("running"),
    partition: info.label ?? "–",
    contents: contentsLabel(info.contents),
    choices: lock === undefined ? downloadChoices(info, flash) : [],
  };
  if (info.layout !== null && info.layout !== undefined) row.layout = matchLabel(info.layout.status, info.layout.fields);
  if (lock !== undefined) row.lock = lock;
  return row;
}

/** The type, subtype and flags of a layout entry, in words. */
export function partitionFacts(info: RegionInfo): { kind: string; flags: string } | undefined {
  const partition = info.partition;
  if (partition === null || partition === undefined) return undefined;
  const hex = (value: number): string => `0x${value.toString(16).padStart(2, "0")}`;
  return {
    kind: `${typeName(partition.type)} ${subtypeName(partition.type, partition.subtype)} (${hex(partition.type)} / ${hex(partition.subtype)})`,
    flags: `0x${partition.flags.toString(16).padStart(8, "0")}${isReadOnly(partition.flags) ? ", read-only" : ""}`,
  };
}

/** What `DownloadChoice` asks for, for the download queue. */
export function choiceWhat(key: "whole" | "image"): DownloadWhat {
  return key === "image" ? { kind: "image" } : { kind: "whole" };
}

/** A firmware card, in words. */
export interface FirmwareView {
  title: string;
  badges: { label: string; accent: boolean }[];
  /** Set when the slot is selected but didn't start, so the bootloader fell back from it. */
  fellBack?: string;
  status: Labelled;
  fault?: string;
  name?: string;
  built?: string;
  idf?: string;
  secureVersion?: string;
  /** The image's length against the slot's size. */
  fill?: { share: number; text: string };
  canDownloadImage: boolean;
}

/** The card of an app slot. */
export function firmwareView(app: AppSlot, flash: InternalFlash, regions: readonly RegionInfo[]): FirmwareView {
  const role = app.region === "app_current" ? "Current" : app.region === "app_alternate" ? "Alternate" : app.region;
  const roles = regions.find((info) => info.region === app.region)?.roles ?? [];
  const view: FirmwareView = {
    title: app.slot === null ? role : `${role} (${app.slot})`,
    badges: roles.map((r) => ({ label: roleLabel(r), accent: r === "running" })),
    status: imageStatusLabel(app.image.status),
    canDownloadImage: app.image.size !== null && app.image.size !== undefined && app.image.status !== "empty",
  };
  const boot = flash.boot;
  if (boot?.needs_confirmation === true && roles.includes("running")) {
    view.badges.push({ label: "Needs confirmation", accent: false });
  }
  // A slot selected by an entry that is still `new` holds an update waiting for the next boot, not one that failed.
  const selecting = flash.otadata
    .filter((entry) => entry.validity === "valid" && entry.selects === app.slot)
    .sort((a, b) => (b.seq ?? 0) - (a.seq ?? 0))[0];
  if (
    roles.includes("selected") &&
    !roles.includes("running") &&
    boot !== null &&
    boot !== undefined &&
    selecting?.state !== "new"
  ) {
    view.fellBack = `The otadata selects this slot, and the bootloader started ${boot.running} instead: it fell back from this one.`;
  }
  if (app.image.error !== null && app.image.error !== undefined) view.fault = faultLabel(app.image.error);
  const descriptor = app.image.descriptor;
  if (descriptor !== null && descriptor !== undefined) {
    view.name = `${descriptor.project_name} ${descriptor.version}`;
    view.built = `${descriptor.date} ${descriptor.time}`;
    view.idf = descriptor.idf_version;
    view.secureVersion = String(descriptor.secure_version);
  }
  const size = app.image.size;
  if (size !== null && size !== undefined && app.size > 0) {
    view.fill = { share: size / app.size, text: `${formatBytes(size)} of ${formatBytes(app.size)}` };
  }
  return view;
}

/** A line of the boot chain panel. */
export interface ChainLine {
  label: string;
  text: string;
  level?: StatusLevel;
}

/** The boot chain: the bootloader, the table, what booted and the checks. */
export function bootChain(flash: InternalFlash): ChainLine[] {
  const { bootloader, table, boot } = flash;
  const lines: ChainLine[] = [];
  const status = bootloaderStatusLabel(bootloader.status);
  lines.push({
    label: "Bootloader",
    text: [
      bootloader.name ?? "Not one this firmware knows",
      status.label,
      bootloader.error === null || bootloader.error === undefined ? undefined : faultLabel(bootloader.error),
    ]
      .filter((part) => part !== undefined)
      .join(" · "),
    level: status.level,
  });
  const built = [
    bootloader.idf_version === null || bootloader.idf_version === undefined ? undefined : `ESP-IDF ${bootloader.idf_version}`,
    bootloader.version === null || bootloader.version === undefined ? undefined : `version ${bootloader.version}`,
    bootloader.built === null || bootloader.built === undefined ? undefined : `built ${bootloader.built}`,
    bootloader.size === null || bootloader.size === undefined ? undefined : formatBytes(bootloader.size),
  ].filter((part) => part !== undefined);
  if (built.length > 0) lines.push({ label: "Built", text: built.join(" · ") });
  else if (bootloader.status !== "invalid") lines.push({ label: "Built", text: "No descriptor (ESP-IDF before 5.3)" });

  const read =
    table.board_offset === null || table.board_offset === undefined
      ? "which table the bootloader read isn't known"
      : table.board_offset === table.offset
        ? "the bootloader read it there"
        : `the bootloader read the one at ${formatHex(table.board_offset, 4)}`;
  lines.push({
    label: "Table",
    text: `${formatHex(table.offset, 4)}, ${table.matches ? "matches the layout" : "doesn't match the layout"}; ${read} (MD5 not checked)`,
    level: table.matches ? "normal" : "caution",
  });

  const bootedSlot = flash.apps.find((app) => app.offset === flash.booted_offset)?.slot;
  const booted =
    flash.booted_offset === null || flash.booted_offset === undefined
      ? "Not known"
      : `${formatHex(flash.booted_offset)}${bootedSlot === null || bootedSlot === undefined ? "" : ` = ${bootedSlot}`}`;
  const selects =
    boot === null || boot === undefined
      ? "the otadata isn't read in this tier"
      : boot.selected === null || boot.selected === undefined
        ? "no otadata entry is valid"
        : boot.selected === boot.running
          ? "as the otadata selects"
          : `the otadata selects ${boot.selected}`;
  lines.push({ label: "Booted", text: `${booted}, ${selects}` });
  if (boot?.state !== null && boot?.state !== undefined) {
    lines.push({ label: "State", text: otaStateLabel(boot.state), level: boot.needs_confirmation ? "caution" : "normal" });
  }
  lines.push({
    label: "Checked",
    text:
      flash.checks.status === "done"
        ? `At boot, finished ${flash.checks.finished_uptime_s ?? 0} s after start`
        : flash.checks.status === "running"
          ? "Checking the app slots now"
          : `"${flash.checks.status}"`,
    level: flash.checks.status === "running" ? "standby" : "normal",
  });
  return lines;
}
