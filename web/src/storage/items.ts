import { bootFacts, isApp, roleChips } from "./boot.ts";
import type { BootFacts, RoleChip } from "./boot.ts";
import { GAP_IDENTITY, GAP_SWATCH, WORDS, consequence, extraIdentity, regionIdentity, searchSentence } from "./copy.ts";
import type { Identity } from "./copy.ts";
import { bootloaderStatus, extraStatus, gapStatus, partitionStatus, tableFindings, tableStatus } from "./status.ts";
import type { Comparison, ItemStatus } from "./status.ts";
import { blockName, stripWeights } from "./strip.ts";
import type {
  BoardTable,
  EntryField,
  InternalFlash,
  RegionInfo,
  StorageDevices,
  TableEntry,
  TableMd5,
  TableRead,
} from "./types.ts";

/** What the storage page is built from, as the storage service holds it. */
export interface StorageInput {
  flash: InternalFlash;
  regions: readonly RegionInfo[];
  /** The board's table; `undefined` while it hasn't been read, which counts as not known. */
  table?: BoardTable | undefined;
  devices?: StorageDevices | undefined;
}

/**
 * What an item of the strip and the list is:
 * - `region`: a region of the expected layout, on the board (or not compared);
 * - `missing`: a partition of the expected layout the board's table has no entry for;
 * - `gap`: space no expected region and no extra entry covers;
 * - `extra`: a board's entry the expected layout doesn't have.
 */
export type ItemKind = "region" | "gap" | "missing" | "extra";

/** What a region holds, as far as the page knows: `none` for a gap, `unknown` for an extra entry or a value the page doesn't know. */
export type ItemContents = "public" | "sensitive" | "reserved" | "none" | "unknown";

/** How the block, the swatch and the ruler are drawn. */
export type Texture = "public" | "sensitive" | "reserved" | "gap" | "missing" | "extra";

/** A range of flash: `start` up to, but not including, `end`. */
export interface Range {
  start: number;
  end: number;
  size: number;
}

/** Another item that shares bytes with this one on the board. */
export interface Overlap {
  id: string;
  title: string;
  subtitle: string;
  colour: string;
  /** The shared range: from the later start to the earlier end. */
  start: number;
  end: number;
  size: number;
}

/** A board's table entry with its index in the table. */
export interface BoardEntry extends TableEntry {
  index: number;
}

/** A region, gap, missing region or extra entry, as the strip, the list and the inspector show it. */
export interface Item extends Identity {
  /**
   * The selection key, kept across refreshes: `bootloader`, `partition_table`, `label:<label>` for a
   * partition, `region:<region>` for one without a label, `gap:<start>`, `extra:<table index>`.
   */
  id: string;
  kind: ItemKind;
  /** The text of the list's and the inspector's swatch: the code, or an en dash for a gap. */
  swatch: string;
  /** The expected region (region and missing). */
  info?: RegionInfo;
  /** The board's entry: the one matched by label (region), or the extra one. */
  entry?: BoardEntry;
  contents: ItemContents;
  texture: Texture;
  /** Whether the block carries the lock: a sensitive region. */
  lock: boolean;
  /** The span drawn on the strip and the ruler: the expected span, or an extra entry's. */
  span: Range;
  /** Where the item is on the board, for overlaps: its board entry's span, else its expected span. */
  place?: Range;
  roles: RoleChip[];
  /** The fields in which the board's entry differs from the expected layout. */
  differs: EntryField[];
  status: ItemStatus;
  overlaps: Overlap[];
  /** For a gap that is "Used", the labels of the shifted entries in it. */
  usedBy: string[];
  /** The used share of the region for the usage bar, when it has one. */
  usage?: number;
  /** The block's `flex-grow`, and its share of the strip. */
  grow: number;
  share: number;
  /** The block's accessible name. */
  name: string;
}

/** The storage page, as data. */
export interface StorageModel {
  input: StorageInput;
  /** The flash size: the chip's, or the expected layout's when the chip's isn't known. */
  flashSize: number;
  /** Whether the chip's size is known. */
  sizeKnown: boolean;
  /** The end of the strip: the larger of the flash size and the highest end of any block. */
  stripEnd: number;
  comparison: Comparison;
  /** The board's table, when the firmware knows it. */
  board: TableRead | null;
  /** The MD5 the page reports: the board's table's, else the one at the expected offset. */
  md5: TableMd5 | null;
  /** The tier's consequence sentence; empty in the full tier. */
  consequence: string;
  /** The search sentence, when the board's table isn't known. */
  search: string;
  boot: BootFacts;
  /** The partition table's findings, when it differs. */
  findings: string[];
  /** Every item, in strip order. */
  items: Item[];
  /** The minimum `grow` of a block. */
  minimum: number;
}

const ENTRY_FIELDS: readonly EntryField[] = ["type", "subtype", "offset", "size", "flags"];

function range(offset: number, size: number): Range {
  return { start: offset, end: offset + size, size };
}

/** The shared range of two ranges, or `undefined` when they share no byte. */
export function intersect(a: Range, b: Range): Range | undefined {
  const start = Math.max(a.start, b.start);
  const end = Math.min(a.end, b.end);
  return start < end ? { start, end, size: end - start } : undefined;
}

/** The selection key of an expected region. */
export function regionId(info: RegionInfo): string {
  if (info.region === "bootloader" || info.region === "partition_table") return info.region;
  return info.label === null ? `region:${info.region}` : `label:${info.label}`;
}

function contentsOf(contents: string): ItemContents {
  return contents === "public" || contents === "sensitive" || contents === "reserved" ? contents : "unknown";
}

function textureOf(contents: ItemContents): Texture {
  return contents === "sensitive" || contents === "reserved" ? contents : "public";
}

/** The entry's fields that differ from the expected region's. */
function differingFields(info: RegionInfo, span: Range, entry: TableEntry): EntryField[] {
  const partition = info.partition;
  if (partition === null || partition === undefined) return [];
  const expected = {
    type: partition.type,
    subtype: partition.subtype,
    offset: span.start,
    size: span.size,
    flags: partition.flags,
  };
  return ENTRY_FIELDS.filter((field) => field !== "label" && expected[field as keyof typeof expected] !== entry[field]);
}

interface Draft {
  id: string;
  kind: ItemKind;
  identity: Identity;
  info?: RegionInfo;
  entry?: BoardEntry;
  span: Range;
  place?: Range;
  differs: EntryField[];
  /** For ordering: expected items before gaps before extra entries at the same start. */
  rank: number;
  order: number;
}

/** The space in `[0, end)` no range covers. */
function uncovered(ranges: readonly Range[], end: number): Range[] {
  const sorted = ranges.filter((r) => r.size > 0).sort((a, b) => a.start - b.start);
  const gaps: Range[] = [];
  let cursor = 0;
  for (const r of sorted) {
    if (r.start > cursor) gaps.push(range(cursor, r.start - cursor));
    cursor = Math.max(cursor, r.end);
  }
  if (end > cursor) gaps.push(range(cursor, end - cursor));
  return gaps;
}

/** The used share of a region for its usage bar, when it has one. */
function usageOf(info: RegionInfo, span: Range, flash: InternalFlash, board: TableRead | null): number | undefined {
  if (span.size <= 0) return undefined;
  let used: number | undefined;
  if (isApp(info)) {
    const image = flash.apps.find((app) => app.region === info.region)?.image;
    if (image?.status === "valid" && image.size !== null && image.size !== undefined) used = image.size;
  } else if (info.region === "bootloader") {
    used = flash.bootloader.size ?? undefined;
  } else if (info.region === "partition_table") {
    if (board !== null) used = (board.entries.length + (board.md5 === null ? 0 : 1)) * 32;
  } else if (info.region === "otadata") {
    used = span.size;
  }
  return used === undefined ? undefined : Math.min(1, used / span.size);
}

/**
 * Builds the page's model: the expected regions, the board's entries matched
 * to them by label, the gaps, the overlaps, every item's status, and the
 * strip's widths.
 */
export function buildModel(input: StorageInput): StorageModel {
  const { flash, regions } = input;
  const boot = bootFacts(flash);
  const noSlot = regions.some((info) => info.region === "app_current" && info.label === null) || flash.limit === "not_a_slot";
  const board = input.table?.table ?? null;
  const comparison: Comparison = noSlot ? "no-slot" : board === null ? "unknown" : "compare";
  const tierSentence = consequence(flash.tier, flash.limit);
  const flashSize = flash.capacity > 0 ? flash.capacity : flash.layout_size;
  const search =
    board === null && input.table !== undefined
      ? searchSentence(input.table.search, input.table.offsets, flash.booted_offset ?? null)
      : "";

  // The expected regions, each with the board's entry of its label.
  const entries: BoardEntry[] = comparison === "compare" && board !== null ? board.entries.map((e, index) => ({ ...e, index })) : [];
  const claimed = new Set<number>();
  const drafts: Draft[] = [];
  regions.forEach((info, order) => {
    const span = info.expected === null || info.expected === undefined ? range(info.offset, info.size) : range(info.expected.offset, info.expected.size);
    const draft: Draft = { id: regionId(info), kind: "region", identity: regionIdentity(info.region, info.label), info, span, differs: [], rank: 0, order };
    const chain = info.region === "bootloader" || info.region === "partition_table";
    if (comparison === "compare") {
      if (chain) draft.place = span;
      else if (info.partition !== null && info.partition !== undefined && info.label !== null) {
        const entry = entries.find((candidate) => candidate.label === info.label);
        if (entry === undefined) {
          draft.kind = "missing";
          draft.place = span;
        } else {
          claimed.add(entry.index);
          draft.entry = entry;
          draft.place = range(entry.offset, entry.size);
          draft.differs = differingFields(info, span, entry);
        }
      }
    }
    drafts.push(draft);
  });
  // A board's entry no expected region claimed is extra, as is a later entry with a claimed label.
  for (const entry of entries) {
    if (claimed.has(entry.index)) continue;
    const span = range(entry.offset, entry.size);
    drafts.push({ id: `extra:${entry.index}`, kind: "extra", identity: extraIdentity(entry.label), entry, span, place: span, differs: [], rank: 2, order: entry.index });
  }

  const blocksEnd = drafts.reduce((max, draft) => Math.max(max, draft.span.end), 0);
  const stripEnd = Math.max(flashSize, blocksEnd);
  for (const gap of uncovered(
    drafts.map((draft) => draft.span),
    stripEnd,
  )) {
    drafts.push({ id: `gap:${gap.start}`, kind: "gap", identity: GAP_IDENTITY, span: gap, differs: [], rank: 1, order: gap.start });
  }
  drafts.sort((a, b) => a.span.start - b.span.start || a.rank - b.rank || a.order - b.order);

  // Overlaps between places, stated on both sides.
  const overlaps = new Map<string, Overlap[]>(drafts.map((draft) => [draft.id, []]));
  for (let i = 0; i < drafts.length; i += 1) {
    for (let j = i + 1; j < drafts.length; j += 1) {
      const a = drafts[i];
      const b = drafts[j];
      if (a?.place === undefined || b?.place === undefined) continue;
      const shared = intersect(a.place, b.place);
      if (shared === undefined) continue;
      const side = (other: Draft): Overlap => ({
        id: other.id,
        title: other.identity.title,
        subtitle: other.identity.subtitle,
        colour: other.identity.colour,
        ...shared,
      });
      overlaps.get(a.id)?.push(side(b));
      overlaps.get(b.id)?.push(side(a));
    }
  }

  // The shifted entries reaching into each gap.
  const usedBy = (gap: Range): string[] =>
    drafts.flatMap((draft) =>
      draft.kind === "region" && draft.entry !== undefined && draft.place !== undefined && intersect(draft.place, gap) !== undefined
        ? [draft.entry.label]
        : [],
    );

  const weights = stripWeights(
    drafts.map((draft) => draft.span.size),
    stripEnd,
  );

  const items: Item[] = drafts.map((draft, index) => {
    const theirs = overlaps.get(draft.id) ?? [];
    const contents: ItemContents =
      draft.kind === "gap" ? "none" : draft.kind === "extra" || draft.info === undefined ? "unknown" : contentsOf(draft.info.contents);
    const texture: Texture =
      draft.kind === "gap" ? "gap" : draft.kind === "missing" ? "missing" : draft.kind === "extra" ? "extra" : textureOf(contents);
    const item: Item = {
      ...draft.identity,
      id: draft.id,
      kind: draft.kind,
      swatch: draft.kind === "gap" ? GAP_SWATCH : draft.identity.code,
      contents,
      texture,
      lock: texture === "sensitive",
      span: draft.span,
      roles: draft.info === undefined ? [] : roleChips(draft.info, boot),
      differs: draft.differs,
      status: { level: "ok", word: "", reason: "" },
      overlaps: theirs,
      usedBy: draft.kind === "gap" && comparison === "compare" ? usedBy(draft.span) : [],
      grow: weights.grow[index] ?? 0,
      share: weights.share[index] ?? 0,
      name: "",
    };
    if (draft.info !== undefined) item.info = draft.info;
    if (draft.entry !== undefined) item.entry = draft.entry;
    if (draft.place !== undefined) item.place = draft.place;
    if (draft.kind === "region" && draft.info !== undefined) {
      const usage = usageOf(draft.info, draft.span, flash, input.table?.table ?? null);
      if (usage !== undefined) item.usage = usage;
    }
    return item;
  });

  // Statuses: every item but the partition table first, as its findings come from them.
  for (const item of items) {
    const info = item.info;
    switch (item.kind) {
      case "gap":
        item.status = gapStatus(comparison, item.usedBy);
        break;
      case "extra":
        item.status = extraStatus(item.entry ?? { label: item.title, type: 0, subtype: 0 }, item.overlaps, tierSentence);
        break;
      case "region":
      case "missing":
        if (info === undefined || info.region === "partition_table") break;
        if (info.region === "bootloader") {
          item.status = bootloaderStatus(flash.bootloader, item.overlaps, tierSentence);
        } else if ((info.partition !== null && info.partition !== undefined) || comparison !== "compare") {
          item.status = partitionStatus({
            comparison,
            label: item.title,
            onBoard: item.kind === "region",
            differs: item.differs,
            overlaps: item.overlaps,
            consequence: tierSentence,
          });
        } else {
          // A region the page doesn't know, outside the partitions.
          item.status = { level: "warn", word: WORDS.unknown, reason: item.explanation };
        }
        break;
    }
  }

  const matched = items.filter((item) => item.kind === "region" && item.entry !== undefined);
  const byTable = [...matched].sort((a, b) => (a.entry?.index ?? 0) - (b.entry?.index ?? 0));
  const findings =
    comparison === "compare" && board !== null
      ? tableFindings({
          items: items.map((item) => ({
            kind: item.kind,
            title: item.title,
            boot: item.id === "bootloader" || item.id === "partition_table",
            differs: item.differs,
            overlaps: item.overlaps,
          })),
          // The matched items are in strip order, which is the expected layout's.
          orderDiffers: matched.some((item, i) => byTable[i] !== item),
          firmwareMatches: flash.table.matches,
          readAt: board.offset,
          expectedAt: flash.table.offset,
          malformed: board.end === "malformed",
        })
      : [];
  const md5 = board !== null ? board.md5 ?? null : flash.table.md5 ?? null;
  const table = items.find((item) => item.id === "partition_table");
  if (table !== undefined) {
    table.status = tableStatus({ comparison, md5, findings, search, consequence: tierSentence, overlaps: table.overlaps });
  }
  for (const item of items) item.name = blockName(item);

  return {
    input,
    flashSize,
    sizeKnown: flash.capacity > 0,
    stripEnd,
    comparison,
    board,
    md5,
    consequence: tierSentence,
    search,
    boot,
    findings,
    items,
    minimum: weights.minimum,
  };
}

/** The item selected by default: the running app's region, else the first. */
export function defaultSelection(model: StorageModel): string | undefined {
  return (
    model.items.find((item) => item.info !== undefined && item.roles.some((chip) => chip.role === "running"))?.id ??
    model.items[0]?.id
  );
}

/** The item to select after a refresh: the same key when it is still there, else the default. */
export function keepSelection(model: StorageModel, id: string | undefined): string | undefined {
  return id !== undefined && model.items.some((item) => item.id === id) ? id : defaultSelection(model);
}

/** The item with the key. */
export function findItem(model: StorageModel, id: string): Item | undefined {
  return model.items.find((item) => item.id === id);
}
