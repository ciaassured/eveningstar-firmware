import type { StatusLevel } from "../ui/StatusIcon.tsx";
import type {
  BootloaderStatus,
  Contents,
  EntryField,
  EntryValidity,
  ImageFault,
  ImageStatus,
  Limit,
  MatchStatus,
  Open,
  OtaState,
  RegionInfo,
  Role,
  SlotName,
  StorageStatus,
  TableEnd,
  TableSearch,
  Tier,
} from "./types.ts";

/**
 * What the storage endpoints say, in words. The values are the ones the API
 * names today; a device on newer firmware may send others, which are shown as
 * they came.
 */
export interface Labelled {
  level: StatusLevel;
  label: string;
}

export function tierLabel(tier: Open<Tier>): Labelled {
  switch (tier) {
    case "full":
      return { level: "normal", label: "Full tier" };
    case "data":
      return { level: "caution", label: "Data tier" };
    case "untrusted":
      return { level: "serious", label: "Untrusted tier" };
    default:
      return { level: "off", label: `Tier "${tier}"` };
  }
}

/** Why the flash is below the full tier, as a sentence. */
export function limitText(limit: Open<Limit>): string {
  switch (limit) {
    case "table":
      return "The partition table on the flash isn't this firmware's layout.";
    case "flash_size":
      return "The flash is smaller than this firmware's layout.";
    case "not_a_slot":
      return "The firmware didn't start from one of the layout's app slots.";
    case "other_table":
      return "The bootloader isn't one this firmware knows, and it may have read another partition table.";
    case "bootloader_invalid":
      return "The bootloader doesn't verify.";
    case "bootloader_unlisted":
      return "The bootloader isn't one this firmware knows to support rolling back an update.";
    case "app_descriptor":
      return "The running firmware's descriptor doesn't read.";
    default:
      return `The device reports the limit "${limit}".`;
  }
}

/** Whether the limit is what flashing with `cargo run`, with its own bootloader and table, leads to. */
export function limitFromDevelopmentFlash(limit: Open<Limit> | null): boolean {
  return limit === "table" || limit === "not_a_slot" || limit === "other_table";
}

export function roleLabel(role: Open<Role>): string {
  switch (role) {
    case "running":
      return "Running";
    case "selected":
      return "Selected for next boot";
    case "rollback_target":
      return "Rollback target";
    default:
      return `Role "${role}"`;
  }
}

export function imageStatusLabel(status: Open<ImageStatus>): Labelled {
  switch (status) {
    case "checking":
      return { level: "standby", label: "Checking" };
    case "valid":
      return { level: "normal", label: "Valid" };
    case "empty":
      return { level: "off", label: "Empty" };
    case "invalid":
      return { level: "serious", label: "Invalid" };
    default:
      return { level: "off", label: `Status "${status}"` };
  }
}

export function faultLabel(fault: Open<ImageFault>): string {
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
      return "A segment's length isn't a multiple of 4";
    case "segment_past_end":
      return "A segment runs past the end of the region";
    case "segment_mapping":
      return "A segment can't be mapped from where it lies";
    case "checksum":
      return "The checksum doesn't match";
    case "no_digest":
      return "No SHA-256 digest appended";
    case "digest_mismatch":
      return "The SHA-256 digest doesn't match";
    case "missing_descriptor":
      return "No app descriptor";
    default:
      return `Fault "${fault}"`;
  }
}

export function bootloaderStatusLabel(status: Open<BootloaderStatus>): Labelled {
  switch (status) {
    case "known":
      return { level: "normal", label: "Known, supports rollback" };
    case "unlisted":
      return { level: "caution", label: "Verifies, not known to support rollback" };
    case "invalid":
      return { level: "serious", label: "Doesn't verify" };
    default:
      return { level: "off", label: `Status "${status}"` };
  }
}

export function otaStateLabel(state: Open<OtaState>): string {
  switch (state) {
    case "new":
      return "New, not booted yet";
    case "pending_verify":
      return "Booted, waiting to be confirmed";
    case "valid":
      return "Confirmed";
    case "invalid":
      return "Marked as not working";
    case "aborted":
      return "Rolled back from";
    case "undefined":
      return "No state written";
    case "other":
      return "A state ESP-IDF doesn't define";
    default:
      return `State "${state}"`;
  }
}

export function validityLabel(validity: Open<EntryValidity>): Labelled {
  switch (validity) {
    case "valid":
      return { level: "normal", label: "Valid" };
    case "blank":
      return { level: "off", label: "Blank" };
    case "corrupt":
      return { level: "caution", label: "CRC doesn't match" };
    case "invalid":
      return { level: "off", label: "Marked invalid" };
    case "aborted":
      return { level: "off", label: "Marked aborted" };
    case "unreadable":
      return { level: "serious", label: "Unreadable" };
    default:
      return { level: "off", label: `"${validity}"` };
  }
}

export function contentsLabel(contents: Open<Contents>): string {
  switch (contents) {
    case "public":
      return "Public";
    case "sensitive":
      return "Sensitive";
    case "reserved":
      return "Reserved";
    default:
      return `"${contents}"`;
  }
}

export function fieldLabel(field: Open<EntryField>): string {
  switch (field) {
    case "label":
      return "label";
    case "type":
      return "type";
    case "subtype":
      return "subtype";
    case "offset":
      return "offset";
    case "size":
      return "size";
    case "flags":
      return "flags";
    default:
      return field;
  }
}

/** Words joined as a list, such as `offset and size`. */
export function joinWords(words: readonly string[]): string {
  if (words.length <= 1) return words[0] ?? "";
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1] ?? ""}`;
}

export function matchLabel(status: Open<MatchStatus>, fields: readonly Open<EntryField>[]): Labelled {
  switch (status) {
    case "matches":
      return { level: "normal", label: "Matches" };
    case "differs":
      return {
        level: "caution",
        label: fields.length === 0 ? "Differs" : `Differs in ${joinWords(fields.map(fieldLabel))}`,
      };
    case "missing":
      return { level: "serious", label: "Missing" };
    default:
      return { level: "off", label: `"${status}"` };
  }
}

export function storageStatusLabel(status: Open<StorageStatus>): Labelled {
  switch (status) {
    case "available":
      return { level: "normal", label: "Available" };
    case "not_supported":
      return { level: "off", label: "Off" };
    default:
      return { level: "off", label: `"${status}"` };
  }
}

export function tableSearchText(search: Open<TableSearch>): string {
  switch (search) {
    case "compiled":
      return "It matches the layout, and the bootloader is known to read it there.";
    case "found":
      return "One table below the running firmware has an app where it started; that is the one shown.";
    case "several":
      return "Several tables below the running firmware have an app where it started, so which one the bootloader read isn't known.";
    case "none":
      return "No table below the running firmware has an app where it started.";
    default:
      return `The device reports the search "${search}".`;
  }
}

export function tableEndText(end: Open<TableEnd>): string {
  switch (end) {
    case "md5":
      return "Ends at an MD5 entry (not checked)";
    case "erased":
      return "Ends at an erased entry";
    case "malformed":
      return "Ends at an entry that isn't valid";
    case "unterminated":
      return "Has no end within its longest length";
    case "read_failed":
      return "An entry didn't read";
    default:
      return `Ends with "${end}"`;
  }
}

export function slotLabel(slot: Open<SlotName> | null | undefined): string {
  return slot ?? "no slot";
}

/** The ESP-IDF name of a partition type. */
export function typeName(type: number): string {
  switch (type) {
    case 0:
      return "app";
    case 1:
      return "data";
    case 2:
      return "bootloader";
    case 3:
      return "partition_table";
    default:
      return hexByte(type);
  }
}

const DATA_SUBTYPES: Record<number, string> = {
  0x00: "ota",
  0x01: "phy",
  0x02: "nvs",
  0x03: "coredump",
  0x04: "nvs_keys",
  0x05: "efuse",
  0x06: "undefined",
  0x80: "esphttpd",
  0x81: "fat",
  0x82: "spiffs",
  0x83: "littlefs",
};

/** The ESP-IDF name of a partition subtype, which depends on its type. */
export function subtypeName(type: number, subtype: number): string {
  if (type === 0) {
    if (subtype === 0x00) return "factory";
    if (subtype >= 0x10 && subtype <= 0x1f) return `ota_${subtype - 0x10}`;
    if (subtype === 0x20) return "test";
  }
  if (type === 1) {
    const name = DATA_SUBTYPES[subtype];
    if (name !== undefined) return name;
  }
  return hexByte(subtype);
}

/** Whether the read-only flag (bit 1) is set. */
export function isReadOnly(flags: number): boolean {
  return (flags & 0b10) !== 0;
}

function hexByte(value: number): string {
  return `0x${value.toString(16).padStart(2, "0")}`;
}

/** Why a region can't be downloaded, or `undefined` when it can. */
export function lockReason(region: RegionInfo, tier: Open<Tier>): string | undefined {
  switch (region.contents) {
    case "public":
      if (region.available) return undefined;
      return tier === "untrusted" ? "Not readable in the untrusted tier" : "Not readable this boot";
    case "sensitive":
      return "Holds configuration, secrets or factory data; never leaves the device";
    case "reserved":
      return "Reserved for later firmware";
    default:
      return "Not a region this page can download";
  }
}
