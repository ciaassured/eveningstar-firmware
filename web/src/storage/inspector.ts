import { formatDecimal, formatHex, formatPercent1, formatSize, formatSpan } from "../lib/format.ts";
import type { SpanText } from "../lib/format.ts";
import { bootLine, bootingRecord, isApp } from "./boot.ts";
import {
  ACTIONS,
  AS_EXPECTED,
  DETAILS,
  EXPECTED,
  EXTRA,
  MISSING,
  NONE,
  NOTES,
  OVERLAP,
  ROLES,
  USAGE,
  WHY_NOTE,
  crcText,
  faultText,
  hashHelpLabel,
  inspectorLabel,
  otaStateText,
  pillName,
} from "./copy.ts";
import type { HelpKey, Level } from "./copy.ts";
import type { Item, ItemKind, StorageModel, Texture } from "./items.ts";
import type { DownloadTarget } from "./plan.ts";
import type { ItemStatus } from "./status.ts";
import type { EntryField, RegionInfo } from "./types.ts";

/** How a run of text is set. */
export type Tone = "plain" | "strong" | "muted" | "mono" | Level;

/** A run of text within a value. */
export interface Run {
  text: string;
  tone: Tone;
}

/** A fact: a label over a value, with an optional status icon before the value. */
export interface Cell {
  label: string;
  icon?: Level;
  runs: Run[];
}

/** A row of muted text with an optional icon, in place of the downloads, or under a panel's content. */
export interface Note {
  icon: "lock" | "warn" | null;
  text: string;
}

/** A download button. */
export interface Action {
  key: "image" | "whole" | "table" | "otadata";
  title: string;
  sublabel: string;
  primary: boolean;
  disabled: boolean;
  target: DownloadTarget;
}

/** How a part of the ruler, and its key square in the span table, is filled. */
export type Fill = "used" | "alternate" | "unused";

/** A part of a region on the ruler, to scale. */
export interface RulerPart {
  key: string;
  /** The label in the span table, such as "Unused, erased". */
  label: string;
  /** The label on the ruler, such as "Unused". */
  short: string;
  fill: Fill;
  /** Its offset in the flash and size. */
  offset: number;
  size: number;
  /** Where it starts and how wide it is, as shares of the ruler. */
  start: number;
  width: number;
  /** Whether it is wide enough (over 18 % of the ruler) to carry its label. */
  labelled: boolean;
  /** The label inside it: the short label in bold, then its size and, for an app image, the share of the region. */
  caption: { short: string; size: string; percent?: string };
  tooltip: string;
}

/** A boundary under the ruler. */
export interface Tick {
  offset: number;
  text: string;
  /** Its place on the ruler, as a share. */
  position: number;
  /** The first row, or the second for a tick that crowds the previous one of the first row. */
  row: 1 | 2;
  align: "start" | "center" | "end";
}

/** A row of a span table. */
export interface SpanRow {
  /** `region`, a part's key, or `overlap:<other id>`. */
  key: string;
  label: string;
  /** The key square's fill: the region's tint, a part's fill, or another region's colour. */
  square: "region" | Fill | "other";
  /** For an overlap row, the other region's key and colour. */
  other?: { id: string; colour: string };
  span: SpanText;
}

/** The usage summary panel: the ruler, its ticks and the span table. */
export interface UsageView {
  title: string;
  texture: Texture;
  parts: RulerPart[];
  /** For a region without parts, the one label over the whole ruler: "<text>, <size>". */
  text?: string;
  ticks: Tick[];
  /** The span table's accessible name. */
  tableLabel: string;
  rows: SpanRow[];
}

/** A role box of an app slot's details. */
export interface RoleBox {
  label: string;
  value: "yes" | "no" | "unknown";
  text: string;
  icon?: "booted" | "next" | "rollback";
}

/** A hash with its label and what it covers. */
export interface HashRow {
  key: "file_sha256" | "digest" | "elf_sha256" | "md5";
  label: string;
  help?: HelpKey;
  /** The help button's label, when the row has one. */
  helpLabel?: string;
  under: string;
  value: string;
  /** The field's accessible name: "<label>, <text under the label>". */
  name: string;
}

export interface HashesView {
  title: string;
  note: string;
  /** Whether the note follows the title on its line, above the rows (an image's hashes), or follows the rows. */
  lead: boolean;
  rows: HashRow[];
}

/** A record of the otadata. */
export interface OtadataRow {
  sector: string;
  crc: { text: string; match: boolean };
  seq: string;
  state: string;
  selects: string;
  /** Whether the bootloader boots this record: bold, and "(next boot)" after its Selects. */
  boots: boolean;
}

export interface OtadataView {
  columns: readonly string[];
  rows: OtadataRow[];
  /** What each state the records use means. */
  states: { state: string; text: string }[];
  /** The muted line on how the bootloader picks a record. */
  rule: string;
}

/** The details panel. */
export interface DetailsView {
  title: string;
  roleBoxes?: RoleBox[];
  cells: Cell[];
  /** Notes under the cells: a `note` box, or a `muted` line. */
  notes: { text: string; tone: "note" | "muted" }[];
  otadata?: OtadataView;
  hashes?: HashesView;
}

/** A value of the comparison table: the main text over an optional muted one. */
export interface CompareValue {
  main: string;
  sub?: string;
  mono: boolean;
}

export interface CompareRow {
  field: EntryField;
  expected: CompareValue;
  board: CompareValue;
  differs: boolean;
}

/** A span on a placement bar, as shares of the bar. */
export interface PlacementBar {
  start: number;
  width: number;
  caption: string;
}

export interface PlacementView {
  caption: string;
  expected: PlacementBar;
  board: PlacementBar;
}

/** The expected state panel, shown when the level isn't ok. */
export interface ExpectedView {
  title: string;
  help: string;
  status: ItemStatus;
  comparison?: { columns: readonly string[]; rows: CompareRow[] };
  placement?: PlacementView;
}

/** The overlap panel. */
export interface OverlapView {
  title: string;
  lines: string[];
  tableLabel: string;
  rows: SpanRow[];
}

/** The panel of a missing region ("Expected layout") or an extra entry ("Board's table entry"). */
export interface EntryPanel {
  title: string;
  cells: Cell[];
  tableLabel: string;
  rows: SpanRow[];
}

/** Everything the inspector shows of an item, in words. */
export interface InspectorView {
  id: string;
  kind: ItemKind;
  /** The article's accessible name. */
  label: string;
  title: string;
  subtitle: string;
  swatch: string;
  colour: string;
  texture: Texture;
  status: ItemStatus;
  /** The status pill: "As expected" when ok, else the status word. */
  pill: { word: string; name: string };
  /** The Why popover. */
  why: { word: string; reason: string; note: string };
  explanation: string;
  bootLine?: string;
  actions: Action[];
  /** The note in place of the downloads. */
  note?: Note;
  usage?: UsageView;
  details?: DetailsView;
  expected?: ExpectedView;
  entryPanel?: EntryPanel;
  overlap?: OverlapView;
}

const LABEL_SHARE = 0.18;
const TICK_GAP = 0.16;
const SECTOR = 0x1000;
const ENTRY_BYTES = 32;

function run(text: string, tone: Tone = "plain"): Run {
  return { text, tone };
}

function regionRow(span: { start: number; size: number }): SpanRow {
  return { key: "region", label: USAGE.region, square: "region", span: formatSpan(span.start, span.size) };
}

// Actions.

function actionsOf(item: Item, model: StorageModel): { actions: Action[]; note?: Note } {
  const info = item.info;
  if (item.kind === "gap") return { actions: [], note: { icon: null, text: NOTES.gap } };
  if (item.kind === "extra") return { actions: [], note: { icon: "lock", text: NOTES.extra } };
  if (item.kind === "missing" || info === undefined) {
    return { actions: [], note: { icon: "warn", text: MISSING.note(item.title, model.consequence) } };
  }
  switch (item.contents) {
    case "sensitive":
      return { actions: [], note: { icon: "lock", text: NOTES.sensitive } };
    case "reserved":
      return { actions: [], note: { icon: null, text: NOTES.reserved } };
    case "public":
      break;
    default:
      return { actions: [], note: { icon: "lock", text: NOTES.extra } };
  }
  if (!info.available) return { actions: [], note: { icon: null, text: NOTES.notAvailable } };
  const { flash } = model.input;
  const region = info.region;
  const whole: Action = {
    key: "whole",
    title: ACTIONS.whole,
    sublabel: ACTIONS.raw(info.size),
    primary: false,
    disabled: false,
    target: { region, what: { kind: "whole" } },
  };
  const image = (title: string, sublabel: string, disabled: boolean): Action => ({
    key: "image",
    title,
    sublabel,
    primary: true,
    disabled,
    target: { region, what: { kind: "image" } },
  });
  if (isApp(info)) {
    const found = flash.apps.find((app) => app.region === region)?.image;
    const size = found?.size;
    const button =
      found?.status === "valid" && size !== null && size !== undefined
        ? image(ACTIONS.image, ACTIONS.imageReady(size), false)
        : found?.status === "checking"
          ? image(ACTIONS.image, ACTIONS.checking, true)
          : image(ACTIONS.image, ACTIONS.noImage, true);
    return { actions: [button, whole] };
  }
  switch (region) {
    case "bootloader": {
      const size = flash.bootloader.size;
      const button =
        size === null || size === undefined
          ? image(ACTIONS.bootloader, ACTIONS.bootloaderInvalid, true)
          : image(ACTIONS.bootloader, ACTIONS.imageReady(size), false);
      return { actions: [button, whole] };
    }
    case "partition_table":
      return { actions: [{ ...whole, key: "table", title: ACTIONS.table, sublabel: ACTIONS.tableFile(info.size), primary: true }] };
    case "otadata":
      return { actions: [{ ...whole, key: "otadata", title: ACTIONS.otadata, sublabel: ACTIONS.otadataFile(info.size), primary: true }] };
    default:
      return { actions: [{ ...whole, primary: true }] };
  }
}

// The usage summary.

interface PartDraft {
  key: string;
  label: string;
  short: string;
  fill: Fill;
  size: number;
}

/** The region's parts, in order, or the text over the whole ruler for a region without parts. */
function partsOf(item: Item, model: StorageModel): PartDraft[] | string {
  const info = item.info;
  if (item.kind === "gap") return USAGE.gap;
  if (info === undefined) return USAGE.unknownRegion;
  switch (item.contents) {
    case "sensitive":
      return USAGE.sensitive(info.region);
    case "reserved":
      return USAGE.reserved;
    case "public":
      break;
    default:
      return USAGE.notRead;
  }
  const size = item.span.size;
  const { flash } = model.input;
  const imageParts = (label: string, length: number): PartDraft[] => [
    { key: "image", label, short: USAGE.image, fill: "used", size: Math.min(length, size) },
    ...(length < size
      ? [{ key: "unused", label: USAGE.unused, short: USAGE.unusedShort, fill: "unused" as const, size: size - length }]
      : []),
  ];
  if (isApp(info)) {
    const image = flash.apps.find((app) => app.region === info.region)?.image;
    if (image === undefined) return USAGE.notRead;
    switch (image.status) {
      case "valid":
        return image.size === null || image.size === undefined ? USAGE.notValid : imageParts(USAGE.image, image.size);
      case "empty":
        return USAGE.erased;
      case "checking":
        return USAGE.checking;
      default:
        return USAGE.notValid;
    }
  }
  switch (info.region) {
    case "bootloader": {
      const length = flash.bootloader.size;
      return length === null || length === undefined ? USAGE.notValid : imageParts(USAGE.bootloaderImage, length);
    }
    case "otadata":
      return [
        { key: "sector0", label: USAGE.sector(0), short: USAGE.sectorShort(0), fill: "used", size: Math.min(SECTOR, size) },
        {
          key: "sector1",
          label: USAGE.sector(1),
          short: USAGE.sectorShort(1),
          fill: "alternate",
          size: Math.max(0, Math.min(SECTOR, size - SECTOR)),
        },
      ];
    case "partition_table": {
      const board = model.input.table?.table;
      if (board === null || board === undefined) return USAGE.notKnown;
      const count = board.entries.length;
      const entries = Math.min(count * ENTRY_BYTES, size);
      const md5 = board.md5 === null ? 0 : Math.min(ENTRY_BYTES, size - entries);
      const parts: PartDraft[] = [{ key: "entries", label: USAGE.entries(count), short: USAGE.entriesShort, fill: "used", size: entries }];
      if (md5 > 0) parts.push({ key: "md5", label: USAGE.md5, short: USAGE.md5Short, fill: "alternate", size: md5 });
      if (size - entries - md5 > 0) {
        parts.push({ key: "unused", label: USAGE.unused, short: USAGE.unusedShort, fill: "unused", size: size - entries - md5 });
      }
      return parts;
    }
    default:
      return USAGE.unknownRegion;
  }
}

/**
 * The ticks under the ruler: the region's start, each part's end and the
 * region's end, without duplicates. A tick within 16 % of the ruler after the
 * previous tick of the first row goes to the second row; one that also crowds
 * the previous tick of the second row is left out, except the last.
 */
export function rulerTicks(start: number, size: number, ends: readonly number[]): Tick[] {
  const offsets = [...new Set([start, ...ends, start + size])].sort((a, b) => a - b);
  const ticks: Tick[] = [];
  let lastFirst: number | undefined;
  let lastSecond: number | undefined;
  offsets.forEach((offset, i) => {
    const position = size > 0 ? (offset - start) / size : 0;
    const last = i === offsets.length - 1;
    const align = i === 0 ? "start" : last ? "end" : "center";
    const tick = (row: 1 | 2): Tick => ({ offset, text: formatHex(offset), position, row, align });
    if (lastFirst === undefined || position - lastFirst >= TICK_GAP) {
      ticks.push(tick(1));
      lastFirst = position;
    } else if (lastSecond === undefined || position - lastSecond >= TICK_GAP || last) {
      ticks.push(tick(2));
      lastSecond = position;
    }
  });
  return ticks;
}

function usageOf(item: Item, model: StorageModel): UsageView {
  const { start, size } = item.span;
  const found = partsOf(item, model);
  const parts: RulerPart[] = [];
  if (typeof found !== "string") {
    let cursor = start;
    for (const part of found) {
      if (part.size <= 0) continue;
      const width = size > 0 ? part.size / size : 0;
      const caption: RulerPart["caption"] = { short: part.short, size: formatSize(part.size) };
      if (part.key === "image" && item.info !== undefined && isApp(item.info)) caption.percent = USAGE.ofRegion(formatPercent1(width));
      parts.push({
        ...part,
        offset: cursor,
        start: size > 0 ? (cursor - start) / size : 0,
        width,
        labelled: width > LABEL_SHARE,
        caption,
        tooltip: USAGE.tooltip(part.label, cursor, cursor + part.size),
      });
      cursor += part.size;
    }
  }
  const view: UsageView = {
    title: USAGE.title,
    texture: item.texture,
    parts,
    ticks: rulerTicks(
      start,
      size,
      parts.map((part) => part.offset + part.size),
    ),
    tableLabel: USAGE.spans(item.title),
    rows: [
      regionRow(item.span),
      ...parts.map((part) => ({ key: part.key, label: part.label, square: part.fill, span: formatSpan(part.offset, part.size) })),
    ],
  };
  if (typeof found === "string") view.text = USAGE.whole(found, size);
  return view;
}

// Details.

function hashRow(key: HashRow["key"], label: string, under: string, value: string, help?: HelpKey): HashRow {
  const row: HashRow = { key, label, under, value, name: `${label}, ${under}` };
  if (help !== undefined) {
    row.help = help;
    row.helpLabel = hashHelpLabel(label);
  }
  return row;
}

function appDetails(item: Item, info: RegionInfo, model: StorageModel): DetailsView {
  const { flash } = model.input;
  const known = model.boot.boot !== null;
  const has = (role: string): boolean => item.roles.some((chip) => chip.role === role);
  const box = (label: string, icon: NonNullable<RoleBox["icon"]>, yes: boolean, isKnown: boolean): RoleBox =>
    !isKnown
      ? { label, value: "unknown", text: ROLES.notKnown }
      : yes
        ? { label, value: "yes", text: ROLES.yes, icon }
        : { label, value: "no", text: ROLES.no };
  const roleBoxes = [
    box(ROLES.running, "booted", has("running"), true),
    box(ROLES.selected, "next", has("selected"), known),
    box(ROLES.rollback, "rollback", has("rollback_target"), known),
  ];
  const details: DetailsView = { title: DETAILS.title, roleBoxes, cells: [], notes: [] };
  const image = flash.apps.find((app) => app.region === info.region)?.image;
  if (image === undefined) {
    details.cells.push({ label: DETAILS.image, runs: [run(DETAILS.notRead, "muted")] });
    return details;
  }
  switch (image.status) {
    case "valid": {
      const checked = image.checked_uptime_s;
      details.cells.push({
        label: DETAILS.image,
        icon: "ok",
        runs: [run(DETAILS.valid, "ok"), ...(checked === null || checked === undefined ? [] : [run(DETAILS.checkedAfter(checked), "muted")])],
      });
      const descriptor = image.descriptor;
      if (descriptor !== null && descriptor !== undefined) {
        details.cells.push(
          { label: DETAILS.projectAndVersion, runs: [run(`${descriptor.project_name} `), run(descriptor.version, "strong")] },
          { label: DETAILS.built, runs: [run(`${descriptor.date}, ${descriptor.time}`)] },
          { label: DETAILS.builtWith, runs: [run(descriptor.idf_version)] },
          { label: DETAILS.secureVersion, runs: [run(String(descriptor.secure_version))] },
        );
      }
      const size = image.size;
      if (size !== null && size !== undefined && image.file_sha256 && image.digest) {
        const rows = [
          hashRow("file_sha256", DETAILS.fileSha256, DETAILS.fileSha256Under(size), image.file_sha256, "file_sha256"),
          hashRow("digest", DETAILS.digest, DETAILS.digestUnder(size), image.digest, "stored_digest"),
        ];
        if (descriptor !== null && descriptor !== undefined) {
          rows.push(hashRow("elf_sha256", DETAILS.elf, DETAILS.elfUnder, descriptor.elf_sha256));
        }
        details.hashes = { title: DETAILS.hashes, note: DETAILS.hashesApp, lead: true, rows };
      }
      break;
    }
    case "empty":
      details.cells.push({ label: DETAILS.image, runs: [run(DETAILS.empty)] });
      break;
    case "invalid": {
      const fault = image.error;
      details.cells.push({
        label: DETAILS.image,
        icon: "bad",
        runs: [run(DETAILS.notValid, "bad"), ...(fault === null || fault === undefined ? [] : [run(` (${faultText(fault)})`, "muted")])],
      });
      if (item.entry !== undefined && item.entry.offset !== item.span.start) {
        details.notes.push({ text: DETAILS.shifted(item.title), tone: "note" });
      }
      break;
    }
    case "checking":
      details.cells.push({ label: DETAILS.image, runs: [run(DETAILS.checking, "muted")] });
      break;
    default:
      details.cells.push({ label: DETAILS.image, runs: [run(image.status)] });
  }
  return details;
}

function bootloaderDetails(item: Item, model: StorageModel): DetailsView {
  const { bootloader } = model.input.flash;
  const cells: Cell[] = [
    { label: DETAILS.status, icon: item.status.level, runs: [run(item.status.word, item.status.level)] },
    {
      label: DETAILS.knownAs,
      runs: [bootloader.name === null || bootloader.name === undefined ? run(DETAILS.notListed, "muted") : run(bootloader.name, "mono")],
    },
  ];
  const idf = bootloader.idf_version;
  if (idf !== null && idf !== undefined && !(bootloader.name ?? "").includes(idf)) cells.push({ label: DETAILS.idf, runs: [run(idf)] });
  if (bootloader.version !== null && bootloader.version !== undefined) {
    cells.push({ label: DETAILS.bootloaderVersion, runs: [run(String(bootloader.version))] });
  }
  if (bootloader.built !== null && bootloader.built !== undefined) cells.push({ label: DETAILS.built, runs: [run(bootloader.built)] });
  const details: DetailsView = { title: DETAILS.title, cells, notes: [] };
  const size = bootloader.size;
  if (size !== null && size !== undefined && bootloader.file_sha256 && bootloader.digest) {
    details.hashes = {
      title: DETAILS.hashes,
      note: DETAILS.hashesBootloader,
      lead: true,
      rows: [
        hashRow("file_sha256", DETAILS.fileSha256, DETAILS.fileSha256Under(size), bootloader.file_sha256, "file_sha256"),
        hashRow("digest", DETAILS.digest, DETAILS.digestUnder(size), bootloader.digest, "stored_digest"),
      ],
    };
  }
  return details;
}

function tableDetails(model: StorageModel): DetailsView {
  const { board, md5 } = model;
  const { flash } = model.input;
  const count = board === null ? null : board.entries.length;
  const cells: Cell[] = [
    {
      label: DETAILS.entries,
      runs: [
        count === null
          ? run(DETAILS.notKnown, "muted")
          : model.comparison !== "compare"
            ? run(String(count))
            : run(model.findings.length === 0 ? DETAILS.entriesMatch(count) : DETAILS.entriesDiffer(count)),
      ],
    },
  ];
  const md5Label = board === null ? DETAILS.md5CheckAt(flash.table.offset) : DETAILS.md5Check;
  if (md5 === null) cells.push({ label: md5Label, runs: [run(DETAILS.noMd5, "muted")] });
  else if (md5.matches) cells.push({ label: md5Label, icon: "ok", runs: [run(DETAILS.md5Matches, "ok"), run(DETAILS.md5Against, "muted")] });
  else cells.push({ label: md5Label, icon: "bad", runs: [run(DETAILS.md5Differs, "bad"), run(DETAILS.md5Against, "muted")] });
  cells.push({
    label: DETAILS.readFrom,
    runs:
      board === null
        ? [run(DETAILS.notKnown, "muted")]
        : [run(formatHex(board.offset), "mono"), run(` ${formatDecimal(board.offset)}`, "muted")],
  });
  const details: DetailsView = { title: DETAILS.title, cells, notes: [] };
  if (board?.truncated === true) details.notes.push({ text: DETAILS.truncated(board.entries.length), tone: "note" });
  if (md5 !== null) {
    details.hashes = {
      title: DETAILS.hashes,
      note: DETAILS.hashesTable,
      lead: false,
      rows: [hashRow("md5", DETAILS.storedMd5, DETAILS.storedMd5Under(count), md5.stored, "md5")],
    };
  }
  return details;
}

function otadataDetails(model: StorageModel): DetailsView {
  const { otadata } = model.input.flash;
  const details: DetailsView = { title: DETAILS.title, cells: [], notes: [] };
  if (otadata.length === 0) {
    details.notes.push({ text: DETAILS.otadataNotRead, tone: "muted" });
    return details;
  }
  const booting = bootingRecord(otadata);
  const states: string[] = [];
  for (const entry of otadata) {
    if (entry.state !== null && entry.state !== undefined && !states.includes(entry.state)) states.push(entry.state);
  }
  details.otadata = {
    columns: DETAILS.otadataColumns,
    rows: otadata.map((entry) => ({
      sector: String(entry.sector),
      crc: crcText(entry.validity),
      seq: entry.seq === null || entry.seq === undefined ? NONE : String(entry.seq),
      state: entry.state ?? NONE,
      selects: entry.selects ?? NONE,
      boots: entry === booting,
    })),
    states: states.map((state) => ({ state, text: otaStateText(state) })),
    rule: DETAILS.otadataRule,
  };
  details.hashes = { title: DETAILS.hashes, note: DETAILS.hashesOtadata, lead: false, rows: [] };
  return details;
}

function detailsOf(item: Item, model: StorageModel): DetailsView | undefined {
  const info = item.info;
  if (item.kind !== "region" || info === undefined || item.contents !== "public") return undefined;
  if (isApp(info)) return appDetails(item, info, model);
  switch (info.region) {
    case "bootloader":
      return bootloaderDetails(item, model);
    case "partition_table":
      return tableDetails(model);
    case "otadata":
      return otadataDetails(model);
    default:
      return undefined;
  }
}

// The expected state panel.

function compareRows(item: Item): CompareRow[] {
  const info = item.info;
  const entry = item.entry;
  const partition = info?.partition;
  if (info === undefined || entry === undefined || partition === null || partition === undefined) return [];
  const text = (main: string): CompareValue => ({ main, mono: false });
  const hex = (value: number): CompareValue => ({ main: formatHex(value, 0), mono: false });
  const offset = (value: number): CompareValue => ({ main: formatHex(value), sub: formatDecimal(value), mono: true });
  const size = (value: number): CompareValue => ({ main: formatHex(value), sub: formatSize(value), mono: true });
  const row = (field: EntryField, expected: CompareValue, board: CompareValue): CompareRow => ({
    field,
    expected,
    board,
    differs: expected.main !== board.main,
  });
  return [
    row("label", text(info.label ?? ""), text(entry.label)),
    row("type", text(String(partition.type)), text(String(entry.type))),
    row("subtype", hex(partition.subtype), hex(entry.subtype)),
    row("offset", offset(item.span.start), offset(entry.offset)),
    row("size", size(item.span.size), size(entry.size)),
    row("flags", hex(partition.flags), hex(entry.flags)),
  ];
}

function placementOf(item: Item): PlacementView | undefined {
  const board = item.place;
  if (board === undefined) return undefined;
  const low = Math.min(item.span.start, board.start);
  const high = Math.max(item.span.end, board.end);
  const width = high - low;
  const bar = (r: { start: number; size: number }, caption: string): PlacementBar => ({
    start: width > 0 ? (r.start - low) / width : 0,
    width: width > 0 ? r.size / width : 0,
    caption,
  });
  return {
    caption: EXPECTED.placement(low, high),
    expected: bar(item.span, EXPECTED.expectedBar(item.span.start, item.span.end)),
    board: bar(board, EXPECTED.boardBar(board.start, board.end)),
  };
}

function expectedOf(item: Item): ExpectedView | undefined {
  if (item.status.level === "ok" || (item.kind !== "region" && item.kind !== "gap")) return undefined;
  const view: ExpectedView = { title: EXPECTED.title, help: EXPECTED.help, status: item.status };
  if (item.kind === "region" && item.differs.length > 0) {
    view.comparison = { columns: EXPECTED.columns, rows: compareRows(item) };
    const placement = placementOf(item);
    if (placement !== undefined) view.placement = placement;
  }
  return view;
}

// The overlap panel.

function overlapOf(item: Item): OverlapView | undefined {
  if (item.overlaps.length === 0) return undefined;
  const line = item.kind === "missing" ? OVERLAP.lineMissing : OVERLAP.line;
  return {
    title: OVERLAP.title,
    lines: item.overlaps.map((other) => line(other)),
    tableLabel: USAGE.spans(item.title),
    rows: [
      regionRow(item.place ?? item.span),
      ...item.overlaps.map((other) => ({
        key: `overlap:${other.id}`,
        label: OVERLAP.row(other.title),
        square: "other" as const,
        other: { id: other.id, colour: other.colour },
        span: formatSpan(other.start, other.size),
      })),
    ],
  };
}

// The inspector.

/** Everything the inspector shows of the item with the key; `undefined` when there is none. */
export function inspectorView(model: StorageModel, id: string): InspectorView | undefined {
  const item = model.items.find((candidate) => candidate.id === id);
  if (item === undefined) return undefined;
  const ok = item.status.level === "ok";
  const word = ok ? AS_EXPECTED : item.status.word;
  const { actions, note } = actionsOf(item, model);
  const view: InspectorView = {
    id: item.id,
    kind: item.kind,
    label: inspectorLabel(item.title),
    title: item.title,
    subtitle: item.subtitle,
    swatch: item.swatch,
    colour: item.colour,
    texture: item.texture,
    status: item.status,
    pill: { word, name: pillName(word) },
    why: { word, reason: item.status.reason, note: WHY_NOTE },
    explanation: item.kind === "missing" ? MISSING.intro(item.title) : item.explanation,
    actions,
  };
  if (note !== undefined) view.note = note;
  if (item.info !== undefined && item.kind === "region" && isApp(item.info)) {
    view.bootLine = bootLine(item.info, item.roles, model.boot);
  }
  switch (item.kind) {
    case "missing":
      view.entryPanel = { title: MISSING.panel, cells: [], tableLabel: USAGE.spans(item.title), rows: [regionRow(item.span)] };
      break;
    case "extra": {
      const entry = item.entry;
      const byte = (value: number): Run[] => [run(formatHex(value, 2))];
      view.entryPanel = {
        title: EXTRA.panel,
        cells:
          entry === undefined
            ? []
            : [
                { label: EXTRA.name, runs: [run(entry.label)] },
                { label: EXTRA.type, runs: byte(entry.type) },
                { label: EXTRA.subtype, runs: byte(entry.subtype) },
                { label: EXTRA.flags, runs: byte(entry.flags) },
              ],
        tableLabel: USAGE.spans(item.title),
        rows: [regionRow(item.span)],
      };
      break;
    }
    default: {
      view.usage = usageOf(item, model);
      const details = detailsOf(item, model);
      if (details !== undefined) view.details = details;
      const expected = expectedOf(item);
      if (expected !== undefined) view.expected = expected;
    }
  }
  const overlap = overlapOf(item);
  if (overlap !== undefined) view.overlap = overlap;
  return view;
}
