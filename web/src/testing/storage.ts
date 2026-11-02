import { createHash } from "node:crypto";
import type { AppSlot, BoardTable, InternalFlash, RegionInfo, Tier } from "../storage/types.ts";

/** The SHA-256 of the bytes, in lowercase hex. */
export function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

/** Bytes that differ from one `seed` to another. */
export function bytesOf(size: number, seed = 1): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(size);
  let state = seed * 2_654_435_761;
  for (let i = 0; i < size; i += 1) {
    state = (state * 1_103_515_245 + 12_345) >>> 0;
    bytes[i] = state >>> 24;
  }
  return bytes;
}

/** The digest the fixtures' partition tables store in their MD5 entry. */
export const TABLE_MD5 = "c5f387345ce92323fb411bc8bfcf3bce";

/** The board's layout as the regions endpoint lists it, in the full tier with ota_0 running. */
export function regions(options: { tier?: Tier; differs?: boolean } = {}): RegionInfo[] {
  const untrusted = options.tier === "untrusted";
  const layout = (fields: RegionInfo["layout"]): RegionInfo["layout"] => fields;
  const matches = layout({ status: "matches", fields: [] });
  const part = (
    region: RegionInfo["region"],
    label: string,
    type: number,
    subtype: number,
    offset: number,
    size: number,
    contents: RegionInfo["contents"],
    roles: RegionInfo["roles"] = [],
    flags = 0,
  ): RegionInfo => ({
    region,
    label,
    partition: { type, subtype, flags },
    offset,
    size,
    contents,
    available: contents === "public" && (!untrusted || region === "app_current"),
    layout: matches,
    roles,
  });
  const list: RegionInfo[] = [
    { region: "bootloader", label: null, partition: null, offset: 0, size: 0x8000, contents: "public", available: true, layout: null, roles: [] },
    { region: "partition_table", label: null, partition: null, offset: 0x8000, size: 0x1000, contents: "public", available: true, layout: null, roles: [] },
    part("factory", "factory", 1, 6, 0x9000, 0x1000, "sensitive", [], 2),
    part("otadata", "otadata", 1, 0, 0xa000, 0x2000, "public"),
    part("app_current", "ota_0", 0, 0x10, 0x1_0000, 0x30_0000, "public", untrusted ? [] : ["running", "selected"]),
    part("app_alternate", "ota_1", 0, 0x11, 0x31_0000, 0x30_0000, "public"),
    part("config_current", "config_0", 1, 6, 0x61_0000, 0x2_0000, "sensitive"),
    part("config_alternate", "config_1", 1, 6, 0x63_0000, 0x2_0000, "sensitive"),
    part("diag", "diag", 1, 6, 0x65_0000, 0x4_0000, "sensitive"),
    part("reserved", "reserved", 1, 6, 0x69_0000, 0x17_0000, "reserved"),
  ];
  if (options.differs === true) {
    const app = list[5];
    if (app !== undefined) app.layout = { status: "differs", fields: ["offset", "size"] };
    const reserved = list[9];
    if (reserved !== undefined) reserved.layout = { status: "missing", fields: [] };
  }
  return list;
}

/** The current slot's image: its bytes, and both digests. */
export function appImage(size = 4_096, seed = 7) {
  const bytes = bytesOf(size, seed);
  return { bytes, fileSha256: sha256(bytes), digest: sha256(bytes.subarray(0, size - 32)) };
}

function slot(region: AppSlot["region"], slotName: AppSlot["slot"], offset: number, image: AppSlot["image"]): AppSlot {
  return { region, slot: slotName, offset, size: 0x30_0000, image };
}

/** What `/storage/internal` answers in the tier, with `image` in ota_0 and ota_1 empty. */
export function internalFlash(tier: Tier = "full", image = appImage(), overrides: Partial<InternalFlash> = {}): InternalFlash {
  const valid: AppSlot["image"] = {
    status: "valid",
    checked_uptime_s: 3,
    error: null,
    size: image.bytes.byteLength,
    digest: image.digest,
    file_sha256: image.fileSha256,
    descriptor: {
      project_name: "eveningstar",
      version: "0.1.0",
      date: "Oct 31 2026",
      time: "10:12:44",
      idf_version: "esp-hal",
      secure_version: 0,
      elf_sha256: "9a0b".repeat(16),
    },
  };
  const empty: AppSlot["image"] = {
    status: "empty",
    checked_uptime_s: 3,
    error: null,
    size: null,
    digest: null,
    file_sha256: null,
    descriptor: null,
  };
  const bootloader = appImage(22_528, 3);
  const base: InternalFlash = {
    capacity: 0x80_0000,
    layout_size: 0x80_0000,
    tier,
    limit: tier === "full" ? null : tier === "data" ? "bootloader_unlisted" : "table",
    persistent_config: tier !== "untrusted",
    updates: tier === "full",
    booted_offset: 0x1_0000,
    table: {
      offset: 0x8000,
      matches: tier !== "untrusted",
      md5: { stored: TABLE_MD5, matches: tier !== "untrusted" },
      board_offset: tier === "untrusted" ? null : 0x8000,
    },
    bootloader: {
      status: tier === "full" ? "known" : "unlisted",
      name: tier === "full" ? "esp-idf-v5.5.1-rollback" : null,
      error: null,
      size: bootloader.bytes.byteLength,
      digest: bootloader.digest,
      file_sha256: bootloader.fileSha256,
      version: 1,
      idf_version: "v5.5.1",
      built: "Oct 30 2026 12:00:00",
    },
    boot:
      tier === "untrusted"
        ? null
        : { running: "ota_0", selected: "ota_0", state: "valid", rollback_target: false, needs_confirmation: false },
    otadata:
      tier === "untrusted"
        ? []
        : [
            { sector: 0, validity: "valid", seq: 3, state: "valid", selects: "ota_0" },
            { sector: 1, validity: "blank", seq: null, state: null, selects: null },
          ],
    apps:
      tier === "untrusted"
        ? [slot("app_current", "ota_0", 0x1_0000, valid)]
        : [slot("app_current", "ota_0", 0x1_0000, valid), slot("app_alternate", "ota_1", 0x31_0000, empty)],
    checks: { status: "done", finished_uptime_s: 4 },
  };
  return { ...base, ...overrides };
}

/** What `/storage/internal/table` answers: the board's layout, or one with ota_1 moved and an extra entry. */
export function boardTable(differs = false): BoardTable {
  const entries = regions()
    .filter((info) => info.partition !== null)
    .map((info) => ({
      label: info.label ?? "",
      type: info.partition?.type ?? 0,
      subtype: info.partition?.subtype ?? 0,
      offset: info.offset,
      size: info.size,
      flags: info.partition?.flags ?? 0,
    }));
  if (differs) {
    const moved = entries[3];
    if (moved !== undefined) {
      moved.offset = 0x32_0000;
      moved.size = 0x20_0000;
    }
    entries.pop();
  }
  return {
    search: differs ? "found" : "compiled",
    offsets: [0x8000],
    table: { offset: 0x8000, entries, truncated: false, end: "md5", md5: { stored: TABLE_MD5, matches: true } },
  };
}
