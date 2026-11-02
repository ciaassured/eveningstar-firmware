import { FINDINGS, REASONS, WORDS, overlapSentence, reasonTail } from "./copy.ts";
import type { Level, OverlapWords } from "./copy.ts";
import type { BootloaderInfo, EntryField, TableMd5 } from "./types.ts";

/** Whether a region is as the firmware expects, in a word, and why. */
export interface ItemStatus {
  level: Level;
  word: string;
  reason: string;
}

/**
 * How the board's table is compared with the expected layout:
 * - `compare`: by label, against the board's table;
 * - `unknown`: not at all, as the firmware doesn't know which table the bootloader read;
 * - `no-slot`: not at all, as the app didn't boot from one of the expected layout's slots, so the expected layout listed is incomplete.
 */
export type Comparison = "compare" | "unknown" | "no-slot";

const ok = (reason: string): ItemStatus => ({ level: "ok", word: WORDS.expected, reason });
const warn = (word: string, reason: string): ItemStatus => ({ level: "warn", word, reason });

/** A status that is as expected otherwise, with overlaps: the word becomes "Overlap", and the reason gains them. */
function overlapped(status: ItemStatus, overlaps: readonly OverlapWords[], consequence: string): ItemStatus {
  if (overlaps.length === 0) return status;
  if (status.level === "ok") return warn(WORDS.overlap, status.reason + reasonTail(overlaps, consequence));
  return { ...status, reason: status.reason + overlaps.map(overlapSentence).join("") };
}

/** The bootloader's status: known, unlisted, invalid, or one the page doesn't know. */
export function bootloaderStatus(
  bootloader: BootloaderInfo,
  overlaps: readonly OverlapWords[] = [],
  consequence = "",
): ItemStatus {
  let status: ItemStatus;
  switch (bootloader.status) {
    case "known":
      status = ok(REASONS.bootloaderKnown(bootloader.name ?? null));
      break;
    case "unlisted":
      status = warn(WORDS.unknown, REASONS.bootloaderUnlisted);
      break;
    case "invalid":
      status = { level: "bad", word: WORDS.invalid, reason: REASONS.bootloaderInvalid(bootloader.error ?? null) };
      break;
    default:
      status = warn(WORDS.unknown, REASONS.bootloaderOther(bootloader.status));
  }
  return overlapped(status, overlaps, consequence);
}

/** What the partition table's status depends on. */
export interface TableFacts {
  comparison: Comparison;
  /** The MD5 entry of the board's table, or of the table at the expected offset when the board's isn't known. */
  md5: TableMd5 | null;
  findings: readonly string[];
  /** The search sentence, for a board's table that isn't known. */
  search: string;
  consequence: string;
  overlaps?: readonly OverlapWords[];
}

/** The partition table's status. A damaged MD5 comes first, as it says the table itself is broken. */
export function tableStatus(facts: TableFacts): ItemStatus {
  const overlaps = facts.overlaps ?? [];
  if (facts.md5 !== null && !facts.md5.matches) {
    return overlapped({ level: "bad", word: WORDS.md5, reason: REASONS.tableMd5 }, overlaps, facts.consequence);
  }
  switch (facts.comparison) {
    case "no-slot":
      return warn(WORDS.unknown, REASONS.notASlot);
    case "unknown":
      return warn(WORDS.unknown, REASONS.tableUnknown(facts.search));
    case "compare":
      if (facts.findings.length > 0) {
        return warn(
          WORDS.differs,
          REASONS.tableDiffers(facts.findings, facts.md5 !== null, reasonTail(overlaps, facts.consequence)),
        );
      }
      return overlapped(ok(REASONS.tableMatches(facts.md5 !== null)), overlaps, facts.consequence);
  }
}

/** What a partition of the expected layout's status depends on. */
export interface PartitionFacts {
  comparison: Comparison;
  label: string;
  /** Whether the board's table has an entry with the label. */
  onBoard: boolean;
  /** The fields in which the board's entry differs. */
  differs: readonly EntryField[];
  overlaps: readonly OverlapWords[];
  consequence: string;
}

/** A partition's status: matches, differs, missing, overlapped, or not compared. */
export function partitionStatus(facts: PartitionFacts): ItemStatus {
  switch (facts.comparison) {
    case "no-slot":
      return warn(WORDS.unknown, REASONS.notASlot);
    case "unknown":
      return warn(WORDS.unknown, REASONS.partitionUnknown);
    case "compare":
      break;
  }
  if (!facts.onBoard) {
    return warn(
      WORDS.missing,
      REASONS.partitionMissing(
        facts.label,
        facts.overlaps.map((other) => other.title),
        reasonTail([], facts.consequence),
      ),
    );
  }
  if (facts.differs.length > 0) {
    return warn(
      WORDS.differs,
      REASONS.partitionDiffers(facts.label, facts.differs, reasonTail(facts.overlaps, facts.consequence)),
    );
  }
  if (facts.overlaps.length > 0) {
    return warn(WORDS.overlap, REASONS.partitionOverlap(facts.label, facts.overlaps, reasonTail([], facts.consequence)));
  }
  return ok(REASONS.partitionMatches(facts.label));
}

/** A gap's status: unallocated as expected, used by a shifted entry, or not compared. */
export function gapStatus(comparison: Comparison, usedBy: readonly string[]): ItemStatus {
  switch (comparison) {
    case "no-slot":
      return warn(WORDS.unknown, REASONS.notASlot);
    case "unknown":
      return warn(WORDS.unknown, REASONS.gapUnknown);
    case "compare":
      return usedBy.length > 0 ? warn(WORDS.used, REASONS.gapUsed(usedBy)) : ok(REASONS.gapExpected);
  }
}

/** An extra entry's status. */
export function extraStatus(
  entry: { label: string; type: number; subtype: number },
  overlaps: readonly OverlapWords[],
  consequence: string,
): ItemStatus {
  return warn(WORDS.extra, REASONS.extra(entry.label, entry.type, entry.subtype, reasonTail(overlaps, consequence)));
}

/** An item as the partition table's findings see it. */
export interface FindingSubject {
  kind: "region" | "gap" | "missing" | "extra";
  title: string;
  /** Whether it is the bootloader or the partition table, which the findings leave out. */
  boot: boolean;
  differs: readonly EntryField[];
  overlaps: readonly { title: string }[];
}

/** What the reasons of a differing partition table list. */
export interface TableFindingFacts {
  /** The items, in strip order. */
  items: readonly FindingSubject[];
  /** Whether the board's matched entries are in another order than the expected layout's. */
  orderDiffers: boolean;
  /** Whether the firmware's own, positional comparison found the table at the expected offset to match. */
  firmwareMatches: boolean;
  /** Where the bootloader read the board's table, and where the expected layout has it. */
  readAt: number;
  expectedAt: number;
  /** Whether the board's table ends at an entry that isn't valid. */
  malformed: boolean;
}

/**
 * The partition table's findings: one per item not as expected, in offset
 * order, then the order of the entries, where the table was read and how it
 * ends. The order finding also stands for a positional mismatch nothing else
 * explains, so the table never reads "Expected" while the firmware's
 * comparison fails.
 */
export function tableFindings(facts: TableFindingFacts): string[] {
  const findings: string[] = [];
  for (const item of facts.items) {
    if (item.boot) continue;
    switch (item.kind) {
      case "missing":
        findings.push(FINDINGS.missing(item.title));
        break;
      case "extra":
        findings.push(FINDINGS.extra(item.title));
        break;
      case "region":
        if (item.differs.length > 0) findings.push(FINDINGS.differs(item.title));
        else if (item.overlaps.length > 0) {
          findings.push(FINDINGS.overlaps(item.title, item.overlaps.map((other) => other.title)));
        }
        break;
      case "gap":
        break;
    }
  }
  const explained = findings.length > 0 || facts.readAt !== facts.expectedAt || facts.malformed;
  if (facts.orderDiffers || (!explained && !facts.firmwareMatches)) findings.push(FINDINGS.order);
  if (facts.readAt !== facts.expectedAt) findings.push(FINDINGS.readAt(facts.readAt, facts.expectedAt));
  if (facts.malformed) findings.push(FINDINGS.malformed);
  return findings;
}
