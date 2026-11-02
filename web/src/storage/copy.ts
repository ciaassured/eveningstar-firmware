import { formatDecimal, formatHex, formatSize, joinWords } from "../lib/format.ts";
import type { EntryValidity, ImageFault, Limit, Open, OtaState, Region, Tier } from "./types.ts";

/**
 * The words of the storage page: every user-facing text, in the datasheet
 * style the page uses. Values from a newer firmware that the page doesn't
 * know are shown as the API names them.
 */

/** How far a region is from what the firmware expects. */
export type Level = "ok" | "warn" | "bad";

/** What a region is called, and how it looks. */
export interface Identity {
  /** The code on the strip block and the swatch, such as `A0`; empty for a gap. */
  code: string;
  /** The colour token, such as `--r-a0`. */
  colour: string;
  title: string;
  subtitle: string;
  /** The intro of the inspector. */
  explanation: string;
}

const LABEL_LOOKS: Readonly<Record<string, { code: string; colour: string }>> = {
  factory: { code: "FD", colour: "--r-fd" },
  otadata: { code: "OD", colour: "--r-od" },
  ota_0: { code: "A0", colour: "--r-a0" },
  ota_1: { code: "A1", colour: "--r-a1" },
  config_0: { code: "C0", colour: "--r-c0" },
  config_1: { code: "C1", colour: "--r-c1" },
  diag: { code: "DG", colour: "--r-dg" },
  reserved: { code: "RS", colour: "--r-rs" },
};

interface RegionWords {
  /** The labels the region can have; `null` for the regions without one. */
  labels: readonly (string | null)[];
  subtitle: string;
  explanation: string;
}

const REGION_WORDS: Readonly<Record<Region, RegionWords>> = {
  bootloader: {
    labels: [null],
    subtitle: "Second-stage bootloader",
    explanation:
      "The second-stage bootloader handles partition table parsing, boot selection and automatic app rollback. It is loaded by the first-stage bootloader, which is burned into the chip's ROM.",
  },
  partition_table: {
    labels: [null],
    subtitle: "Map of the flash",
    explanation:
      "The partition table lists the flash partitions, each with a name, type, offset and size. The second-stage bootloader reads it on every boot.",
  },
  factory: { labels: ["factory"], subtitle: "Factory data", explanation: "Factory data written during manufacturing." },
  otadata: {
    labels: ["otadata"],
    subtitle: "Boot selection",
    explanation:
      "The OTA data region enables app slot selection with automatic rollback. It holds two boot selection records, each with a sequence number, the selected app slot and an OTA state.",
  },
  app_current: {
    labels: ["ota_0", "ota_1"],
    subtitle: "Running app",
    explanation:
      "The OTA app slot the device is currently running from. It holds one app image; the remainder of the slot is unused.",
  },
  app_alternate: {
    labels: ["ota_0", "ota_1"],
    subtitle: "Other app",
    explanation: "The second OTA app slot. OTA updates are written here and selected for the next boot.",
  },
  config_current: {
    labels: ["config_0", "config_1"],
    subtitle: "Running config",
    explanation: "Settings saved by the running firmware.",
  },
  config_alternate: {
    labels: ["config_0", "config_1"],
    subtitle: "Other config",
    explanation: "Settings for the other app slot, kept so that the slot can roll back with its own configuration.",
  },
  diag: {
    labels: ["diag"],
    subtitle: "Diagnostics",
    explanation: "Diagnostic crash and event records written by the firmware.",
  },
  reserved: { labels: ["reserved"], subtitle: "Reserved", explanation: "Space reserved for future firmware. It is not read." },
};

const BOOT_TITLES: Partial<Record<Region, string>> = { bootloader: "Bootloader", partition_table: "Partition table" };

/** The identity of an expected region. It follows the label, so a slot keeps its code and colour when the other slot boots. */
export function regionIdentity(region: Open<Region>, label: string | null): Identity {
  if (region === "app_current" && label === null) {
    return {
      code: "RA",
      colour: "--r-a0",
      title: "Running app",
      subtitle: "Not an expected slot",
      explanation: "The app the bootloader started, outside the expected layout's OTA app slots.",
    };
  }
  const words = (REGION_WORDS as Readonly<Record<string, RegionWords>>)[region];
  if (words === undefined || !words.labels.includes(label)) {
    return {
      code: "??",
      colour: "--r-rs",
      title: label ?? region,
      subtitle: "Unknown region",
      explanation: "A region this page does not know.",
    };
  }
  const looks = label === null ? undefined : LABEL_LOOKS[label];
  const boot = (BOOT_TITLES as Readonly<Record<string, string>>)[region];
  return {
    code: looks?.code ?? (region === "bootloader" ? "BL" : "PT"),
    colour: looks?.colour ?? (region === "bootloader" ? "--r-bl" : "--r-pt"),
    title: boot ?? label ?? region,
    subtitle: words.subtitle,
    explanation: words.explanation,
  };
}

export const GAP_IDENTITY: Identity = {
  code: "",
  colour: "--r-gap",
  title: "Unallocated",
  subtitle: "Gap between entries",
  explanation: "Not covered by any partition table entry. Nothing is stored here.",
};

/** The text a gap's swatch shows in place of a code. */
export const GAP_SWATCH = "–";

/** The text a value that is `null` shows. */
export const NONE = "–";

/** The intro of an extra entry. */
export function extraIntro(label: string): string {
  return `The board's partition table has a ${label} entry that the expected layout does not have. Its contents are unknown.`;
}

export function extraIdentity(label: string): Identity {
  return { code: "+", colour: "--x-warn", title: label, subtitle: "Extra entry", explanation: extraIntro(label) };
}

// Status words.

export const WORDS = {
  expected: "Expected",
  unknown: "Unknown",
  invalid: "Invalid",
  md5: "MD5 differs",
  differs: "Differs",
  missing: "Missing",
  overlap: "Overlap",
  used: "Used",
  extra: "Extra",
} as const;

/** The word of the inspector's status pill when the level is ok. */
export const AS_EXPECTED = "As expected";

/** The accessible name of the status pill. */
export function pillName(word: string): string {
  return `${word}. Show why`;
}

/** The note under the Why popover's status line. */
export const WHY_NOTE =
  "Whether the partition table on the board has this entry exactly as the expected layout does. If not, the firmware restricts what it does with the flash.";

// Tiers and limits.

/** Why OTA updates are unavailable, completing "because …". */
export function limitClause(limit: Open<Limit> | null): string {
  switch (limit) {
    case "table":
      return "the board's partition table differs from the expected layout";
    case "flash_size":
      return "the flash is smaller than the expected layout";
    case "not_a_slot":
      return "the app did not boot from one of the expected layout's OTA app slots";
    case "other_table":
      return "the bootloader is not in the firmware's list of known bootloaders, and another partition table on the flash could be the one it read";
    case "bootloader_invalid":
      return "the bootloader image fails verification";
    case "bootloader_unlisted":
      return "the bootloader is not in the firmware's list of known bootloaders";
    case "app_descriptor":
      return "the running app's descriptor cannot be read";
    default:
      return "the firmware limits what it does with the flash";
  }
}

/** The consequence sentence of the tier; empty in the full tier. */
export function consequence(tier: Open<Tier>, limit: Open<Limit> | null): string {
  switch (tier) {
    case "full":
      return "";
    case "data":
      return `OTA updates are unavailable because ${limitClause(limit)}.`;
    case "untrusted":
      return `OTA updates are unavailable and settings are not saved because ${limitClause(limit)}.`;
    default:
      return "The firmware limits what it does with the flash.";
  }
}

/** A sentence with a leading space, or nothing. */
function spaced(sentence: string): string {
  return sentence === "" ? "" : ` ${sentence}`;
}

// Faults and OTA states.

/** The short description of an image fault. */
export function faultText(fault: Open<ImageFault>): string {
  switch (fault) {
    case "read_failed":
      return "Reading the flash failed";
    case "past_end":
      return "The image runs past the end of the region";
    case "magic":
      return "No image header";
    case "chip":
      return "Built for another chip";
    case "too_many_segments":
      return "More segments than an image can have";
    case "segment_length":
      return "A segment's length is not a multiple of 4";
    case "segment_past_end":
      return "A segment runs past the end of the region";
    case "segment_mapping":
      return "A segment cannot be mapped from where it lies";
    case "checksum":
      return "The checksum does not match";
    case "no_digest":
      return "No SHA-256 digest appended";
    case "digest_mismatch":
      return "The SHA-256 digest does not match";
    case "missing_descriptor":
      return "No app descriptor";
    default:
      return fault;
  }
}

/** What an OTA state means. */
export function otaStateText(state: Open<OtaState>): string {
  switch (state) {
    case "valid":
      return "Confirmed. The app has booted and marked itself valid. The bootloader does not roll back from a confirmed app.";
    case "new":
      return "Written by an OTA update and selected for the next boot. The app has not been booted.";
    case "pending_verify":
      return "Booted once and awaiting confirmation by the app. If the board restarts before the app confirms, the bootloader rolls back.";
    case "invalid":
    case "aborted":
      return "Rejected. The app did not confirm, and the bootloader rolled back.";
    case "undefined":
      return "No OTA state recorded. The bootloader treats the app as confirmed.";
    case "other":
      return "A value ESP-IDF does not define.";
    default:
      return "A state this page does not know.";
  }
}

// Ranges in sentences.

/** A range in a sentence: `<start> to <end>, <size>`. */
export function rangeWords(start: number, end: number): string {
  return `${formatHex(start)} to ${formatHex(end)}, ${formatSize(end - start)}`;
}

/** Another region and the range it shares with this one. */
export interface OverlapWords {
  title: string;
  subtitle: string;
  start: number;
  end: number;
}

/** The sentence a differing, extra or otherwise flagged region's reason gains per overlap. */
export function overlapSentence(other: OverlapWords): string {
  return ` It overlaps ${other.title} from ${rangeWords(other.start, other.end)}.`;
}

// Reasons.

export const REASONS = {
  bootloaderKnown: (name: string | null) =>
    name === null
      ? "The bootloader is in the firmware's list of known bootloaders and supports rollback."
      : `The bootloader (${name}) is in the firmware's list of known bootloaders and supports rollback.`,
  bootloaderUnlisted:
    "The bootloader image verifies, but the bootloader is not in the firmware's list of known bootloaders. Rollback support cannot be assumed. OTA updates are unavailable.",
  bootloaderInvalid: (fault: Open<ImageFault> | null) =>
    fault === null
      ? "The bootloader image fails verification."
      : `The bootloader image fails verification (${faultText(fault)}).`,
  bootloaderOther: (status: string) => `The device reports the bootloader status ${status}, which this page does not know.`,
  tableMd5: "The stored MD5 does not match the partition table entries. The table may be damaged.",
  tableMatches: (md5: boolean) =>
    md5
      ? "Every entry matches the expected layout, and the stored MD5 matches the entries."
      : "Every entry matches the expected layout. The table has no MD5 entry.",
  tableDiffers: (findings: readonly string[], md5: boolean, tail: string) =>
    md5
      ? `The table is intact (the stored MD5 matches the entries), but it differs from the expected layout: ${findings.join(", ")}.${tail}`
      : `The table has no MD5 entry, and it differs from the expected layout: ${findings.join(", ")}.${tail}`,
  tableUnknown: (search: string) =>
    `The partition table the bootloader read is not known.${spaced(search)}`,
  partitionMatches: (label: string) =>
    `The ${label} entry in the board's table matches the expected layout in type, subtype, offset, size and flags.`,
  partitionDiffers: (label: string, fields: readonly string[], tail: string) =>
    `The ${label} entry in the board's table differs from the expected layout in ${joinWords(fields)}.${tail}`,
  partitionMissing: (label: string, placed: readonly string[], tail: string) =>
    placed.length === 0
      ? `The board's table has no ${label} entry. ${label} is missing from the board, and its expected space is unallocated.${tail}`
      : `The board's table has no ${label} entry. ${label} is missing from the board, and the board's table places ${joinWords(placed)} in its expected space.${tail}`,
  partitionOverlap: (label: string, others: readonly OverlapWords[], tail: string) =>
    `The ${label} entry in the board's table matches the expected layout, but ${others
      .map((other) => `${other.title} overlaps it from ${rangeWords(other.start, other.end)}`)
      .join(", and ")}.${tail}`,
  partitionUnknown: "The partition table the bootloader read is not known, so this entry cannot be compared.",
  gapExpected: "The expected layout leaves this space unallocated. The board's table has no entry here.",
  gapUsed: (labels: readonly string[]) => `The board's table places ${joinWords(labels)} in this space.`,
  gapUnknown: "The partition table the bootloader read is not known, so this space cannot be compared.",
  extra: (label: string, type: number, subtype: number, tail: string) =>
    `The board's table has a ${label} entry (type ${formatHex(type, 2)}, subtype ${formatHex(subtype, 2)}) that the expected layout does not have.${tail}`,
  notASlot: "The app did not boot from one of the expected layout's OTA app slots, so the board's table is not compared.",
} as const;

/** A tail of a reason: the overlap sentences, then the consequence with a leading space. */
export function reasonTail(overlaps: readonly OverlapWords[], consequenceSentence: string): string {
  return overlaps.map(overlapSentence).join("") + spaced(consequenceSentence);
}

/** The findings of a differing partition table, in the order the reason lists them. */
export const FINDINGS = {
  missing: (title: string) => `${title} is missing`,
  differs: (title: string) => `${title} differs`,
  overlaps: (title: string, others: readonly string[]) => `${title} overlaps ${joinWords(others)}`,
  extra: (label: string) => `${label} is extra`,
  order: "the entries are in a different order than in the expected layout",
  readAt: (offset: number, expected: number) =>
    `the bootloader read the table at ${formatHex(offset)}, not at ${formatHex(expected)}`,
  malformed: "the table ends at an entry that is not valid",
} as const;

/** How the firmware searched for the board's table, when it doesn't know it; empty for a search the page doesn't know. */
export function searchSentence(search: string, offsets: readonly number[], bootedOffset: number | null): string {
  switch (search) {
    case "several":
      return `Partition tables were found at ${joinWords(offsets.map((offset) => formatHex(offset)))}, and either could be the one the bootloader read.`;
    case "none":
      return bootedOffset === null
        ? "No partition table on the flash has an app entry at the offset the app booted from."
        : `No partition table on the flash has an app entry at the offset the app booted from (${formatHex(bootedOffset)}).`;
    default:
      return "";
  }
}

// The summary header.

export const SUMMARY = {
  flash: (size: number, known: boolean) =>
    known ? `${formatSize(size)} flash` : `${formatSize(size)} flash (expected size; the chip's size is not known)`,
  matches: "Everything matches the expected layout.",
  update: (label: string) => `An OTA update in ${label} is selected for the next boot.`,
  notExpected: (titles: readonly string[]) =>
    titles.length === 1
      ? `1 item is not as expected: ${titles[0] ?? ""}.`
      : `${titles.length} items are not as expected: ${titles.join(", ")}.`,
  invalidImage: (label: string, fault: Open<ImageFault> | null) =>
    fault === null
      ? `The image in ${label} fails verification.`
      : `The image in ${label} fails verification (${faultText(fault)}).`,
  fallback: (selected: string, running: string) =>
    `The otadata selects ${selected}, but the bootloader failed to load it and started ${running}.`,
  device: (name: string, status: string) =>
    status === "not_supported" ? `${name}: not supported by this firmware.` : `${name}: ${status}.`,
} as const;

/** The name of another storage device. */
export function deviceName(id: string): string {
  return id === "external" ? "External flash" : id === "internal" ? "Internal flash" : id;
}

// The page's frame.

export const PAGE = {
  title: "Storage",
  refresh: "Refresh",
  refreshing: "Refreshing…",
  notRefreshed: (failure: string, time: string) => `Not refreshed: ${failure}. Values from ${time}.`,
  list: "Regions and gaps",
  back: "Back",
  crumb: "Regions",
  downloads: "Downloads",
} as const;

// The strip and the readout.

export const STRIP = {
  label: (flashSize: number) => `Flash map, ${formatSize(flashSize)}, in flash order`,
  help: "How is the flash map drawn?",
} as const;

// Role chips and role boxes.

export const ROLES = {
  running: "Booted",
  selected: "Next boot",
  rollback: "Rollback target",
  fallbackTooltip: "The bootloader failed to load this slot at the last boot.",
  rollbackNextTooltip: "The running app has not confirmed itself, so a restart rolls back to this slot.",
  yes: "Yes",
  no: "No",
  notKnown: "Not known",
} as const;

// Boot lines.

export const BOOT_LINES = {
  runningSelected: "Booted and selected for the next boot.",
  confirmed: "OTA state valid: the app is confirmed, so the bootloader does not roll back.",
  needsConfirmation:
    "OTA state pending_verify: the app has not confirmed itself, so the bootloader rolls back at the next boot.",
  unconfirmedRunning: (other: string) =>
    `Booted. OTA state pending_verify: the app has not confirmed itself, so the next boot rolls back to ${other}.`,
  rollbackNext: (running: string) =>
    `Not running. The app in ${running} has not confirmed itself, so the next boot rolls back to this slot.`,
  runningOtherSelected: (other: string) => `Booted. The next boot starts ${other}.`,
  runningNotRead: "Booted. The otadata is not read in the untrusted tier, so the next boot is not known.",
  runningNothingSelected: "Booted.",
  update: (state: string | null, other: string) =>
    `Selected for the next boot${state === null ? "" : ` (state ${state})`}. The slot has not been booted. After booting, the app must confirm itself, otherwise the bootloader rolls back to ${other}.`,
  fallbackSelected: (other: string) =>
    `Selected for the next boot by the otadata. The bootloader failed to load this slot at the last boot and started ${other} instead.`,
  fallbackRunning: (other: string) =>
    `Booted as a fallback. The otadata selects ${other}, which the bootloader failed to load.`,
  rollback: "The bootloader rolls back to this slot if the app in the other slot fails to confirm itself.",
  none: "Not selected for booting.",
} as const;

// Actions and notes.

export const ACTIONS = {
  image: "Download image",
  bootloader: "Download bootloader",
  table: "Download table",
  otadata: "Download otadata",
  whole: "Whole region",
  added: "Added to downloads",
  imageReady: (size: number) => `${formatSize(size)}, SHA-256 appended inside the file`,
  noImage: "No valid image in this slot",
  checking: "The image check has not finished",
  bootloaderInvalid: "The bootloader image does not verify",
  raw: (size: number) => `${formatSize(size)}, raw bytes`,
  tableFile: (size: number) => `${formatSize(size)} (${formatHex(size, 0)}): entries, MD5 entry and padding in one file`,
  otadataFile: (size: number) => `${formatSize(size)}, both records`,
} as const;

export const NOTES = {
  sensitive: "Raw region downloads are disabled because the region potentially contains secrets, such as cryptographic keys.",
  reserved: "Reserved for future firmware. The region is not read, so no download is offered.",
  gap: "No data is stored in a gap, so no download is offered.",
  notAvailable: "The firmware does not read this region in the untrusted tier, so no download is offered.",
  extra: "Downloads for this region are disabled because the firmware does not know what it contains.",
} as const;

// The usage summary.

export const USAGE = {
  title: "Usage summary",
  spans: (title: string) => `Spans of ${title}`,
  region: "Region",
  headers: { what: "What", start: "Start", end: "End, exclusive", size: "Size" },
  endHelp: "What does end, exclusive mean?",
  ofRegion: (percent: string) => `${percent} of the region`,
  tooltip: (label: string, start: number, end: number) => `${label}: ${rangeWords(start, end)}`,
  bootloaderImage: "Bootloader image",
  image: "Image",
  unused: "Unused, erased",
  unusedShort: "Unused",
  sector: (sector: number) => `Sector ${sector} record`,
  sectorShort: (sector: number) => `Sector ${sector}`,
  entries: (count: number) => (count === 1 ? "1 entry" : `${count} entries`),
  entriesShort: "Entries",
  md5: "MD5 entry",
  md5Short: "MD5",
  erased: "Erased, no image",
  notValid: "No valid image",
  checking: "Image check running",
  notRead: "Not read",
  notKnown: "Not known",
  reserved: "Reserved, unused",
  gap: "Nothing stored",
  unknownRegion: "Unknown region",
  sensitive: (region: Open<Region>) => {
    switch (region) {
      case "factory":
        return "Factory data written during manufacturing";
      case "config_current":
        return "Saved settings of the running app";
      case "config_alternate":
        return "Saved settings of the other app";
      case "diag":
        return "Diagnostic crash and event records";
      default:
        return "Protected data";
    }
  },
  whole: (text: string, size: number) => `${text}, ${formatSize(size)}`,
} as const;

// The details panel.

export const DETAILS = {
  title: "Details",
  image: "Image",
  valid: "Valid",
  checkedAfter: (seconds: number) => ` checked ${seconds} s after boot`,
  projectAndVersion: "Project and version",
  built: "Built",
  builtWith: "Built with",
  secureVersion: "Secure version",
  empty: "None. The slot is erased.",
  notValid: "Not valid",
  checking: "Checking. The background check has not finished.",
  notRead: "Not read. The firmware does not read the other slot in the untrusted tier.",
  shifted: (label: string) =>
    `The data at the expected offset is not an image header. The board's table places ${label} at a different offset, so the image may be located elsewhere. This does not indicate damage.`,
  hashes: "Hashes",
  hashesApp:
    "The SHA-256 appended in the image is part of it and is verified by the bootloader. The SHA-256 of the image file is the value that file hashing tools print for the downloaded file.",
  hashesBootloader:
    "The SHA-256 appended in the image is part of it and is verified by the ROM. The SHA-256 of the image file is the value that file hashing tools print for the downloaded file.",
  fileSha256: "SHA-256 of the image file",
  fileSha256Under: (size: number) => `${formatDecimal(size)} bytes, including the appended SHA-256`,
  digest: "SHA-256 appended in image",
  digestUnder: (size: number) => `validation hash, over the first ${formatDecimal(size - 32)} bytes`,
  elf: "ELF SHA-256",
  elfUnder: "of the ELF file the image was built from",
  status: "Status",
  knownAs: "Known as",
  notListed: "Not in the firmware's list",
  idf: "ESP-IDF",
  bootloaderVersion: "Bootloader version",
  entries: "Entries",
  entriesMatch: (count: number) => `${count}, matching the expected layout`,
  entriesDiffer: (count: number) => `${count}, differing from the expected layout`,
  md5Check: "MD5 check",
  md5CheckAt: (offset: number) => `MD5 check (table at ${formatHex(offset)})`,
  md5Matches: "Matches",
  md5Differs: "Differs",
  md5Against: " stored MD5 against the entries",
  noMd5: "No MD5 entry",
  readFrom: "Read from",
  notKnown: "Not known",
  truncated: (count: number) => `The table has more entries than the device lists (${count}).`,
  storedMd5: "Stored MD5",
  storedMd5Under: (count: number | null) =>
    count === null ? "over the entries, kept in the MD5 entry" : `over the ${count} entries, kept in the MD5 entry`,
  hashesTable: "The table download includes this MD5. The device does not hash the downloaded file as a whole.",
  otadataColumns: ["Sector", "CRC", "Seq", "State", "Selects"],
  crcMatch: "CRC match",
  blank: "Blank, erased",
  crcMismatch: "CRC mismatch",
  unreadable: "Unreadable",
  nextBoot: " (next boot)",
  otadataRule:
    "The bootloader boots the slot named by the record with a matching CRC and the highest sequence number (marked next boot).",
  hashesOtadata:
    "Each record carries its own CRC, which the bootloader verifies. The device does not hash the region as a whole, so the download has no hash.",
  otadataNotRead: "The firmware does not read the otadata in the untrusted tier.",
} as const;

/** What the CRC cell of an otadata record says. */
export function crcText(validity: Open<EntryValidity>): { text: string; match: boolean } {
  switch (validity) {
    case "valid":
    case "invalid":
    case "aborted":
      return { text: DETAILS.crcMatch, match: true };
    case "blank":
      return { text: DETAILS.blank, match: false };
    case "corrupt":
      return { text: DETAILS.crcMismatch, match: false };
    case "unreadable":
      return { text: DETAILS.unreadable, match: false };
    default:
      return { text: validity, match: false };
  }
}

// The expected state panel.

export const EXPECTED = {
  title: "Expected state",
  help: "What does expected mean?",
  columns: ["Field", "Expected layout", "Board's table has"],
  placement: (low: number, high: number) =>
    `Placement of the region, to scale (${formatHex(low)} to ${formatHex(high)})`,
  expectedBar: (start: number, end: number) => `Expected layout: ${formatHex(start)} to ${formatHex(end)}`,
  boardBar: (start: number, end: number) => `Board's table has ${formatHex(start)} to ${formatHex(end)}`,
} as const;

// The overlap panel.

export const OVERLAP = {
  title: "Overlap",
  line: (other: OverlapWords) =>
    `${other.title} (${other.subtitle}) overlaps this region from ${rangeWords(other.start, other.end)}.`,
  lineMissing: (other: OverlapWords) =>
    `The board's table places ${other.title} (${other.subtitle}) in the expected range of this region, from ${rangeWords(other.start, other.end)}.`,
  row: (title: string) => `Overlap with ${title}`,
} as const;

// The missing and extra views.

export const MISSING = {
  intro: (title: string) => `${title} is missing from the board's partition table. The expected layout has it at this range.`,
  note: (label: string, consequenceSentence: string) =>
    `The board's partition table has no ${label} entry, so nothing is described at this range and there is nothing to read or download.${spaced(consequenceSentence)}`,
  panel: "Expected layout",
} as const;

export const EXTRA = {
  panel: "Board's table entry",
  name: "Name",
  type: "Type",
  subtype: "Subtype",
  flags: "Flags",
} as const;

// Help.

export type HelpKey = "span" | "expected" | "file_sha256" | "stored_digest" | "md5" | "strip";

/** The title and text of a help popover; text in backticks is set in the monospace font. */
export function helpTopic(key: HelpKey, flashSize: number): { title: string; text: string } {
  switch (key) {
    case "span":
      return {
        title: "Ranges",
        text: "A range of flash given by an offset and a size. It covers the bytes from the offset up to, but not including, offset + size (end exclusive). Example: a 4 KiB region at 0x009000 ends at 0x00a000, and its last byte is at 0x009fff.",
      };
    case "expected":
      return {
        title: "Expected",
        text: "Whether the partition table on the board has this entry exactly as the expected layout does. If not, the firmware restricts what it does with the flash. For the bootloader, the state means the bootloader is in the firmware's list of known bootloaders. For a gap, it means the board's table has no entry in that space.",
      };
    case "file_sha256":
      return {
        title: "SHA-256 of the image file",
        text: "The SHA-256 of the whole image file, including the appended SHA-256. This is the value `sha256sum`, `Get-FileHash` and `shasum -a 256` print for the downloaded file.",
      };
    case "stored_digest":
      return {
        title: "SHA-256 appended in image",
        text: "The SHA-256 appended to the image, calculated over all preceding bytes. The bootloader and `esptool image_info` call it the validation hash. If it matches, the image is intact.",
      };
    case "md5":
      return {
        title: "Stored MD5",
        text: "The MD5 entry stored after the partition table entries, calculated over those entries. If it matches, the table is intact.",
      };
    case "strip":
      return {
        title: "The flash map",
        text: `The flash map shows every region and gap in flash order, from offset 0x000000 to the end of the ${formatSize(flashSize)} flash. Small regions are drawn wider than their size to remain visible. Within a region, the image is drawn to scale. The ruler in the inspector is drawn to scale. Overlapping entries are drawn side by side, in offset order.`,
      };
  }
}

/** The label of a hash row's help button. */
export function hashHelpLabel(label: string): string {
  return `What is the ${label}?`;
}

/** The label of the inspector. */
export function inspectorLabel(title: string): string {
  return `Inspector: ${title}`;
}

// Downloads in the tray.

export const TRAY = {
  appImage: (title: string) => `App image, ${title}`,
  bootloaderImage: "Bootloader image",
  table: "Partition table, entries and MD5 entry",
  otadata: "Boot selection (otadata)",
  whole: (title: string) => `Whole region, ${title}`,
  imageHash: "SHA-256 of the image file",
  imageHashText: "the value for the file you receive",
  tableHash: "Stored MD5",
  tableHashText: "over the entries; the MD5 entry is part of the file",
  tableNote: "The device does not hash the file as a whole.",
  noHashNote: "No hash: the device does not hash whole regions.",
} as const;
