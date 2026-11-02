import assert from "node:assert/strict";
import { test } from "node:test";
import {
  TABLE_MD5,
  editEntries,
  expectedEntries,
  internalFlash,
  layoutRegions,
  storageInput,
  tableEntry,
} from "../testing/storage.ts";
import type { ScenarioOptions } from "../testing/storage.ts";
import { consequence, limitClause, regionIdentity } from "./copy.ts";
import { buildModel, defaultSelection, findItem, keepSelection, withinSector } from "./items.ts";
import type { Item, StorageModel } from "./items.ts";
import { inspectorView } from "./inspector.ts";
import { summaryView } from "./summary.ts";
import type { InternalFlash, RegionInfo } from "./types.ts";

const UNTRUSTED =
  "OTA updates are unavailable and settings are not saved because the board's partition table differs from the expected layout.";

function model(options: ScenarioOptions = {}): StorageModel {
  return buildModel(storageInput(options));
}

function item(m: StorageModel, id: string): Item {
  const found = findItem(m, id);
  assert.ok(found, `no item ${id}`);
  return found;
}

function words(m: StorageModel): [string, string][] {
  return m.items.map((it) => [it.id, it.status.word]);
}

const LAYOUT_IDS = [
  "bootloader",
  "partition_table",
  "label:factory",
  "label:otadata",
  "gap:49152",
  "label:ota_0",
  "label:ota_1",
  "label:config_0",
  "label:config_1",
  "label:diag",
  "label:reserved",
];

test("a board's table as the expected layout: every item as expected, in offset order, with the gaps the layout leaves", () => {
  const m = model();
  assert.deepEqual(
    m.items.map((it) => it.id),
    LAYOUT_IDS,
  );
  assert.ok(m.items.every((it) => it.status.level === "ok" && it.status.word === "Expected"));
  assert.equal(m.comparison, "compare");
  assert.deepEqual(m.findings, []);
  assert.equal(m.stripEnd, 0x80_0000);

  const gap = item(m, "gap:49152");
  assert.deepEqual(gap.span, { start: 0xc000, end: 0x1_0000, size: 0x4000 });
  assert.equal(gap.kind, "gap");
  assert.equal(gap.code, "");
  assert.equal(gap.swatch, "–");
  assert.equal(gap.texture, "gap");
  assert.equal(gap.status.reason, "The expected layout leaves this space unallocated. The board's table has no entry here.");

  const ota0 = item(m, "label:ota_0");
  assert.equal(ota0.code, "A0");
  assert.equal(ota0.colour, "--r-a0");
  assert.equal(ota0.title, "ota_0");
  assert.equal(ota0.subtitle, "Running app");
  assert.equal(ota0.name, "ota_0, Running app. 0x010000 to 0x310000, 3 MiB. Expected.");
  assert.equal(
    ota0.status.reason,
    "The ota_0 entry in the board's table matches the expected layout in type, subtype, offset, size and flags.",
  );
  assert.deepEqual(ota0.roles, [
    { role: "running", label: "Booted" },
    { role: "selected", label: "Next boot", variant: "plain" },
  ]);

  assert.deepEqual(
    m.items.filter((it) => it.lock).map((it) => it.id),
    ["label:factory", "label:config_0", "label:config_1", "label:diag"],
  );
  assert.equal(item(m, "label:reserved").texture, "reserved");
  assert.deepEqual(
    m.items.filter((it) => it.usage !== undefined).map((it) => it.id),
    ["bootloader", "partition_table", "label:otadata", "label:ota_0"],
  );
  assert.equal(item(m, "partition_table").usage, (9 * 32) / 0xc00);
  assert.equal(item(m, "label:otadata").usage, 1);
  assert.equal(
    item(m, "partition_table").status.reason,
    "Every entry matches the expected layout, and the stored MD5 matches the entries.",
  );
  assert.equal(item(m, "bootloader").status.reason, "The bootloader (esp-idf-v5.5.1-rollback) is in the firmware's list of known bootloaders and supports rollback.");

  const summary = summaryView(m);
  assert.equal(summary.title, "8 MiB flash");
  assert.deepEqual(summary.status, { level: "ok", text: "Everything matches the expected layout." });
  assert.deepEqual(summary.lines, [{ level: "muted", text: "External flash: not supported by this firmware." }]);
});

test("one partition missing at the end, in the untrusted tier: only it is missing, and the table differs", () => {
  const m = model({ tier: "untrusted", entries: editEntries({ reserved: null }) });
  const notOk = m.items.filter((it) => it.status.level !== "ok").map((it) => it.id);
  assert.deepEqual(notOk, ["partition_table", "label:reserved"]);
  const reserved = item(m, "label:reserved");
  assert.equal(reserved.kind, "missing");
  assert.equal(reserved.texture, "missing");
  assert.deepEqual(reserved.span, { start: 0x69_0000, end: 0x80_0000, size: 0x17_0000 });
  assert.equal(reserved.status.word, "Missing");
  assert.equal(
    reserved.status.reason,
    `The board's table has no reserved entry. reserved is missing from the board, and its expected space is unallocated. ${UNTRUSTED}`,
  );
  assert.equal(
    item(m, "partition_table").status.reason,
    `The table is intact (the stored MD5 matches the entries), but it differs from the expected layout: reserved is missing. ${UNTRUSTED}`,
  );
  assert.equal(
    summaryView(m).status.text,
    `2 items are not as expected: Partition table, reserved. ${UNTRUSTED}`,
  );
  // No gap appears where the missing region is expected.
  assert.equal(
    m.items.some((it) => it.kind === "gap" && it.span.start >= 0x69_0000),
    false,
  );
});

test("a missing partition in the middle is reported alone, not as a positional cascade", () => {
  const m = model({ tier: "untrusted", entries: editEntries({ ota_1: null }) });
  assert.deepEqual(
    m.items.filter((it) => it.status.level !== "ok").map((it) => it.id),
    ["partition_table", "label:ota_1"],
  );
  assert.equal(item(m, "label:config_0").status.word, "Expected");
  assert.deepEqual(m.findings, ["ota_1 is missing"]);
});

test("an extra entry in a gap replaces the gap, and is extra with its type and subtype", () => {
  const m = model({ tier: "untrusted", entries: editEntries({}, [tableEntry("scratch", 1, 0x81, 0xc000, 0x4000)]) });
  assert.deepEqual(
    m.items.map((it) => it.id),
    LAYOUT_IDS.map((id) => (id === "gap:49152" ? "extra:8" : id)),
  );
  const scratch = item(m, "extra:8");
  assert.equal(scratch.kind, "extra");
  assert.equal(scratch.code, "+");
  assert.equal(scratch.colour, "--x-warn");
  assert.equal(scratch.subtitle, "Extra entry");
  assert.equal(scratch.contents, "unknown");
  assert.equal(scratch.status.word, "Extra");
  assert.equal(
    scratch.status.reason,
    `The board's table has a scratch entry (type 0x01, subtype 0x81) that the expected layout does not have. ${UNTRUSTED}`,
  );
  assert.deepEqual(m.findings, ["scratch is extra"]);
  const view = inspectorView(m, "extra:8");
  assert.deepEqual(view?.actions, []);
  assert.deepEqual(view?.note, {
    icon: "lock",
    text: "Downloads for this region are disabled because the firmware does not know what it contains.",
  });
  assert.equal(
    view?.explanation,
    "The board's partition table has a scratch entry that the expected layout does not have. Its contents are unknown.",
  );
  assert.deepEqual(
    view?.entryPanel?.cells.map((cell) => [cell.label, cell.runs.map((r) => r.text).join("")]),
    [
      ["Name", "scratch"],
      ["Type", "0x01"],
      ["Subtype", "0x81"],
      ["Flags", "0x00"],
    ],
  );
  assert.equal(view?.entryPanel?.title, "Board's table entry");
  assert.equal(view?.usage, undefined);
});

test("an extra entry that covers part of a gap leaves the rest as gaps", () => {
  const m = model({ entries: editEntries({}, [tableEntry("scratch", 1, 0x81, 0xd000, 0x1000)]) });
  const around = m.items.filter((it) => it.span.start >= 0xc000 && it.span.start < 0x1_0000).map((it) => [it.id, it.span.size]);
  assert.deepEqual(around, [
    ["gap:49152", 0x1000],
    ["extra:8", 0x1000],
    ["gap:57344", 0x2000],
  ]);
});

test("a remainder inside one flash sector is not a gap; a whole sector, or a remainder crossing a sector boundary, is", () => {
  const range = (start: number, end: number) => ({ start, end, size: end - start });
  assert.equal(withinSector(range(0x8c00, 0x9000)), true);
  assert.equal(withinSector(range(0x9000, 0x9400)), true);
  assert.equal(withinSector(range(0x9000, 0xa000)), false);
  assert.equal(withinSector(range(0x8c00, 0x9400)), false);
  // The 1 KiB after the 0xC00-byte partition table is not shown, nor drawn on the strip.
  const m = model();
  assert.equal(
    m.items.some((it) => it.span.start === 0x8c00),
    false,
  );
  assert.equal(m.items.length, LAYOUT_IDS.length);
  // An extra entry ending within a sector leaves no gap behind it in that sector.
  const extra = model({ entries: editEntries({}, [tableEntry("scratch", 1, 0x81, 0xc000, 0x3c00)]) });
  assert.deepEqual(
    extra.items.filter((it) => it.span.start >= 0xc000 && it.span.start < 0x1_0000).map((it) => it.id),
    ["extra:8"],
  );
  // A remainder that crosses a sector boundary is a gap.
  const crossing = model({ entries: editEntries({}, [tableEntry("scratch", 1, 0x81, 0xc000, 0x2c00)]) });
  assert.deepEqual(
    crossing.items.filter((it) => it.kind === "gap" && it.span.start >= 0xc000 && it.span.start < 0x1_0000).map((it) => [it.span.start, it.span.size]),
    [[0xec00, 0x1400]],
  );
});

test("an extra entry overlapping an expected region is its own block after it, and both sides state the overlap", () => {
  const m = model({ tier: "untrusted", entries: editEntries({}, [tableEntry("scratch", 1, 0x81, 0x30_0000, 0x1_0000)]) });
  const ids = m.items.map((it) => it.id);
  assert.equal(ids.indexOf("extra:8"), ids.indexOf("label:ota_0") + 1);
  const ota0 = item(m, "label:ota_0");
  const scratch = item(m, "extra:8");
  assert.equal(ota0.status.word, "Overlap");
  assert.equal(
    ota0.status.reason,
    `The ota_0 entry in the board's table matches the expected layout, but scratch overlaps it from 0x300000 to 0x310000, 64 KiB. ${UNTRUSTED}`,
  );
  assert.equal(scratch.status.word, "Extra");
  assert.equal(
    scratch.status.reason,
    `The board's table has a scratch entry (type 0x01, subtype 0x81) that the expected layout does not have. It overlaps ota_0 from 0x300000 to 0x310000, 64 KiB. ${UNTRUSTED}`,
  );
  assert.deepEqual(m.findings, ["ota_0 overlaps scratch", "scratch is extra"]);

  const panel = inspectorView(m, "label:ota_0")?.overlap;
  assert.deepEqual(panel?.lines, ["scratch (Extra entry) overlaps this region from 0x300000 to 0x310000, 64 KiB."]);
  assert.deepEqual(
    panel?.rows.map((row) => [row.key, row.label, row.span.start.hex, row.span.end.hex, row.span.size.human]),
    [
      ["region", "Region", "0x010000", "0x310000", "3 MiB"],
      ["overlap:extra:8", "Overlap with scratch", "0x300000", "0x310000", "64 KiB"],
    ],
  );
  assert.deepEqual(panel?.rows[1]?.other, { id: "extra:8", colour: "--x-warn" });
  assert.deepEqual(inspectorView(m, "extra:8")?.overlap?.lines, [
    "ota_0 (Running app) overlaps this region from 0x300000 to 0x310000, 64 KiB.",
  ]);
  // The overlapping blocks' sizes add up to more than the flash, and still fill the strip.
  assert.ok(Math.abs(m.items.reduce((sum, it) => sum + it.share, 0) - 1) < 1e-12);
});

test("a shifted entry differs in offset and size, and is drawn at its expected span", () => {
  const m = model({ tier: "untrusted", entries: editEntries({ ota_1: { offset: 0x32_0000, size: 0x20_0000 } }) });
  const ota1 = item(m, "label:ota_1");
  assert.deepEqual(ota1.differs, ["offset", "size"]);
  assert.deepEqual(ota1.span, { start: 0x31_0000, end: 0x61_0000, size: 0x30_0000 });
  assert.deepEqual(ota1.place, { start: 0x32_0000, end: 0x52_0000, size: 0x20_0000 });
  assert.equal(ota1.status.word, "Differs");
  assert.equal(
    ota1.status.reason,
    `The ota_1 entry in the board's table differs from the expected layout in offset and size. ${UNTRUSTED}`,
  );
  const view = inspectorView(m, "label:ota_1");
  assert.deepEqual(
    view?.expected?.comparison?.rows.map((row) => [row.field, row.expected.main, row.expected.sub, row.board.main, row.board.sub, row.differs]),
    [
      ["label", "ota_1", undefined, "ota_1", undefined, false],
      ["type", "0", undefined, "0", undefined, false],
      ["subtype", "0x11", undefined, "0x11", undefined, false],
      ["offset", "0x310000", "3,211,264", "0x320000", "3,276,800", true],
      ["size", "0x300000", "3 MiB", "0x200000", "2 MiB", true],
      ["flags", "0x0", undefined, "0x0", undefined, false],
    ],
  );
  assert.deepEqual(view?.expected?.placement, {
    caption: "Placement of the region, to scale (0x310000 to 0x610000)",
    expected: { start: 0, width: 1, caption: "Expected layout: 0x310000 to 0x610000" },
    board: { start: 1 / 48, width: 2 / 3, caption: "Board's table has 0x320000 to 0x520000" },
  });
  assert.equal(view?.usage?.rows[0]?.span.start.hex, "0x310000");
  assert.equal(view?.usage?.text, "Not read, 3 MiB");
});

test("a shifted entry that reaches into a gap of the expected layout makes it used", () => {
  const m = model({ entries: editEntries({ factory: { offset: 0xc000 } }) });
  const gap = item(m, "gap:49152");
  assert.deepEqual(gap.usedBy, ["factory"]);
  assert.deepEqual(gap.status, { level: "warn", word: "Used", reason: "The board's table places factory in this space." });
  assert.equal(inspectorView(m, "gap:49152")?.expected?.status.word, "Used");
  assert.equal(item(m, "label:factory").status.word, "Differs");
});

test("a resized entry differs in size", () => {
  const m = model({ entries: editEntries({ reserved: { size: 0x10_0000 } }) });
  assert.deepEqual(item(m, "label:reserved").differs, ["size"]);
  assert.deepEqual(m.findings, ["reserved differs"]);
});

test("a missing entry with an extra one in its space: missing then extra, side by side, overlapping", () => {
  const m = model({
    tier: "untrusted",
    entries: editEntries({ ota_1: null }, [tableEntry("storage", 1, 0x82, 0x31_0000, 0x10_0000)]),
  });
  const ids = m.items.map((it) => it.id);
  assert.equal(ids.indexOf("extra:7"), ids.indexOf("label:ota_1") + 1);
  const ota1 = item(m, "label:ota_1");
  assert.equal(ota1.kind, "missing");
  assert.equal(
    ota1.status.reason,
    `The board's table has no ota_1 entry. ota_1 is missing from the board, and the board's table places storage in its expected space. ${UNTRUSTED}`,
  );
  assert.equal(item(m, "extra:7").status.word, "Extra");
  const missing = inspectorView(m, "label:ota_1");
  assert.equal(missing?.explanation, "ota_1 is missing from the board's partition table. The expected layout has it at this range.");
  assert.deepEqual(missing?.note, {
    icon: "warn",
    text: `The board's partition table has no ota_1 entry, so nothing is described at this range and there is nothing to read or download. ${UNTRUSTED}`,
  });
  assert.equal(missing?.entryPanel?.title, "Expected layout");
  assert.deepEqual(
    missing?.entryPanel?.rows.map((row) => row.span.end.hex),
    ["0x610000"],
  );
  assert.equal(missing?.usage, undefined);
  assert.equal(missing?.details, undefined);
  assert.equal(missing?.expected, undefined);
  assert.deepEqual(missing?.overlap?.lines, [
    "The board's table places storage (Extra entry) in the expected range of this region, from 0x310000 to 0x410000, 1 MiB.",
  ]);
  assert.deepEqual(inspectorView(m, "extra:7")?.overlap?.lines, [
    "ota_1 (Other app) overlaps this region from 0x310000 to 0x410000, 1 MiB.",
  ]);
  assert.deepEqual(m.findings, ["ota_1 is missing", "storage is extra"]);
});

test("two expected entries overlapping each other: the resized one differs, the other overlaps", () => {
  const m = model({ tier: "untrusted", entries: editEntries({ diag: { size: 0x5_0000 } }) });
  const diag = item(m, "label:diag");
  const reserved = item(m, "label:reserved");
  assert.equal(diag.status.word, "Differs");
  assert.equal(
    diag.status.reason,
    `The diag entry in the board's table differs from the expected layout in size. It overlaps reserved from 0x690000 to 0x6a0000, 64 KiB. ${UNTRUSTED}`,
  );
  assert.equal(reserved.status.word, "Overlap");
  assert.equal(
    reserved.status.reason,
    `The reserved entry in the board's table matches the expected layout, but diag overlaps it from 0x690000 to 0x6a0000, 64 KiB. ${UNTRUSTED}`,
  );
  assert.deepEqual(m.findings, ["diag differs", "reserved overlaps diag"]);
  assert.equal(
    summaryView(m).status.text,
    `3 items are not as expected: Partition table, diag, reserved. ${UNTRUSTED}`,
  );
});

test("a renamed label is a missing region and an extra entry at the same place", () => {
  const m = model({ tier: "untrusted", entries: editEntries({ ota_1: { label: "ota_1x" } }) });
  assert.equal(item(m, "label:ota_1").kind, "missing");
  const extra = m.items.find((it) => it.kind === "extra");
  assert.equal(extra?.title, "ota_1x");
  assert.match(item(m, "label:ota_1").status.reason, /places ota_1x in its expected space/);
  assert.deepEqual(m.findings, ["ota_1 is missing", "ota_1x is extra"]);
});

test("a wrong type and subtype differ in those fields", () => {
  const m = model({ entries: editEntries({ otadata: { type: 0, subtype: 0x10 } }) });
  const otadata = item(m, "label:otadata");
  assert.deepEqual(otadata.differs, ["type", "subtype"]);
  assert.match(otadata.status.reason, /differs from the expected layout in type and subtype\.$/);
  const rows = inspectorView(m, "label:otadata")?.expected?.comparison?.rows.filter((row) => row.differs);
  assert.deepEqual(
    rows?.map((row) => [row.field, row.expected.main, row.board.main]),
    [
      ["type", "1", "0"],
      ["subtype", "0x0", "0x10"],
    ],
  );
});

test("a later entry with an expected label is extra; the first one belongs to the region", () => {
  const m = model({ entries: editEntries({}, [tableEntry("diag", 1, 6, 0xc000, 0x1000)]) });
  assert.equal(item(m, "label:diag").status.word, "Expected");
  assert.equal(item(m, "extra:8").title, "diag");
});

test("entries in another order, or a positional mismatch nothing else explains, make the table differ", () => {
  const entries = expectedEntries();
  const swapped = [...entries.slice(0, 4), entries[5], entries[4], ...entries.slice(6)].filter((e) => e !== undefined);
  const m = model({ entries: swapped });
  assert.ok(m.items.filter((it) => it.id !== "partition_table").every((it) => it.status.level === "ok"));
  assert.deepEqual(m.findings, ["the entries are in a different order than in the expected layout"]);
  assert.equal(item(m, "partition_table").status.word, "Differs");

  const positional = model({ flash: { table: { offset: 0x8000, matches: false, md5: null, board_offset: 0x8000 } } });
  assert.deepEqual(positional.findings, ["the entries are in a different order than in the expected layout"]);
  assert.notEqual(summaryView(positional).status.text, "Everything matches the expected layout.");
});

test("a table read elsewhere differs, and is read from there", () => {
  const m = model({ tier: "untrusted", boardOffset: 0x6000, board: { search: "found" } });
  assert.deepEqual(m.findings, ["the bootloader read the table at 0x006000, not at 0x008000"]);
  const table = inspectorView(m, "partition_table");
  const readFrom = table?.details?.cells.find((cell) => cell.label === "Read from");
  assert.deepEqual(readFrom?.runs, [
    { text: "0x006000", tone: "mono" },
    { text: " 24,576", tone: "muted" },
  ]);
});

test("a table ending at an entry that is not valid differs", () => {
  const m = model({ read: { end: "malformed" } });
  assert.deepEqual(m.findings, ["the table ends at an entry that is not valid"]);
});

test("a damaged MD5 entry is bad, ahead of any other finding", () => {
  const m = model({ read: { md5: { stored: TABLE_MD5, matches: false } } });
  assert.deepEqual(item(m, "partition_table").status, {
    level: "bad",
    word: "MD5 differs",
    reason: "The stored MD5 does not match the partition table entries. The table may be damaged.",
  });
  const md5 = inspectorView(m, "partition_table")?.details?.cells.find((cell) => cell.label === "MD5 check");
  assert.deepEqual(md5, {
    label: "MD5 check",
    icon: "bad",
    runs: [
      { text: "Differs", tone: "bad" },
      { text: " stored MD5 against the entries", tone: "muted" },
    ],
  });
  assert.equal(summaryView(m).status.text, "1 item is not as expected: Partition table.");
});

test("several candidate tables: nothing is compared, and the summary says which tables were found", () => {
  const input = storageInput({ tier: "untrusted", board: { search: "several", offsets: [0x8000, 0x2_0000], table: null } });
  const m = buildModel(input);
  assert.equal(m.comparison, "unknown");
  const search = "Partition tables were found at 0x008000 and 0x020000, and either could be the one the bootloader read.";
  assert.equal(item(m, "partition_table").status.reason, `The partition table the bootloader read is not known. ${search}`);
  for (const it of m.items) {
    if (it.id === "bootloader") continue;
    assert.equal(it.status.word, "Unknown", it.id);
    assert.equal(it.kind === "missing" || it.kind === "extra", false);
  }
  assert.equal(
    item(m, "label:ota_1").status.reason,
    "The partition table the bootloader read is not known, so this entry cannot be compared.",
  );
  assert.equal(
    item(m, "gap:49152").status.reason,
    "The partition table the bootloader read is not known, so this space cannot be compared.",
  );
  assert.equal(
    summaryView(m).status.text,
    `The partition table the bootloader read is not known. ${search} ${UNTRUSTED}`,
  );
  const details = inspectorView(m, "partition_table")?.details;
  assert.deepEqual(
    details?.cells.map((cell) => [cell.label, cell.runs.map((r) => r.text).join("")]),
    [
      ["Entries", "Not known"],
      ["MD5 check (table at 0x008000)", "Matches stored MD5 against the entries"],
      ["Read from", "Not known"],
    ],
  );
  assert.equal(inspectorView(m, "partition_table")?.usage?.text, "Not known, 3 KiB");
});

test("no table found, or a search the page doesn't know, words the search sentence accordingly", () => {
  const none = buildModel(storageInput({ tier: "untrusted", board: { search: "none", offsets: [], table: null } }));
  assert.equal(
    none.search,
    "No partition table on the flash has an app entry at the offset the app booted from (0x010000).",
  );
  const later = buildModel(storageInput({ tier: "untrusted", board: { search: "guessed" as "none", offsets: [], table: null } }));
  assert.equal(later.search, "");
  assert.equal(item(later, "partition_table").status.reason, "The partition table the bootloader read is not known.");
  const unread = buildModel({ ...storageInput(), table: undefined });
  assert.equal(unread.comparison, "unknown");
});

function noSlotRegions(): RegionInfo[] {
  const list = layoutRegions({ tier: "untrusted" }).filter(
    (info) => !["app_current", "app_alternate", "config_current", "config_alternate"].includes(info.region),
  );
  const running: RegionInfo = {
    region: "app_current",
    label: null,
    partition: null,
    offset: 0x1_0000,
    size: 0x10_0000,
    expected: null,
    contents: "public",
    available: true,
    layout: null,
    roles: ["running"],
  };
  return [...list.slice(0, 4), running, ...list.slice(4)];
}

test("an app that booted from no slot: the running app's block, and nothing compared", () => {
  const flash = { ...internalFlash("untrusted"), limit: "not_a_slot" as const };
  flash.apps = [{ ...flash.apps[0]!, slot: null }];
  const m = buildModel({ ...storageInput({ tier: "untrusted" }), flash, regions: noSlotRegions() });
  assert.equal(m.comparison, "no-slot");
  const running = item(m, "region:app_current");
  assert.equal(running.code, "RA");
  assert.equal(running.title, "Running app");
  assert.equal(running.subtitle, "Not an expected slot");
  assert.deepEqual(running.span, { start: 0x1_0000, end: 0x11_0000, size: 0x10_0000 });
  assert.equal(m.items.some((it) => it.kind === "extra" || it.kind === "missing"), false);
  const reason = "The app did not boot from one of the expected layout's OTA app slots, so the board's table is not compared.";
  for (const it of m.items) {
    if (it.id === "bootloader") continue;
    assert.deepEqual([it.id, it.status.word, it.status.reason], [it.id, "Unknown", reason]);
  }
  assert.equal(
    summaryView(m).status.text,
    `${reason} OTA updates are unavailable and settings are not saved because the app did not boot from one of the expected layout's OTA app slots.`,
  );
  assert.equal(defaultSelection(m), "region:app_current");
  // The table isn't compared, so its entries are counted without saying whether they match.
  const entries = inspectorView(m, "partition_table")?.details?.cells[0];
  assert.deepEqual(entries?.runs, [{ text: "8", tone: "plain" }]);
});

test("a cargo run board: its own entries compared by label, missing, extra and overlapping on both sides", () => {
  const m = model({
    tier: "untrusted",
    entries: [
      tableEntry("nvs", 1, 0x02, 0x9000, 0x6000),
      tableEntry("phy_init", 1, 0x01, 0xf000, 0x1000),
      tableEntry("factory", 0, 0x00, 0x1_0000, 0x3f_0000),
    ],
  });
  assert.deepEqual(words(m), [
    ["bootloader", "Expected"],
    ["partition_table", "Differs"],
    ["label:factory", "Differs"],
    ["extra:0", "Extra"],
    ["label:otadata", "Missing"],
    ["extra:1", "Extra"],
    ["label:ota_0", "Missing"],
    ["label:ota_1", "Missing"],
    ["label:config_0", "Missing"],
    ["label:config_1", "Missing"],
    ["label:diag", "Missing"],
    ["label:reserved", "Missing"],
  ]);
  assert.equal(
    item(m, "label:factory").status.reason,
    `The factory entry in the board's table differs from the expected layout in type, subtype, offset, size and flags. It overlaps ota_0 from 0x010000 to 0x310000, 3 MiB. It overlaps ota_1 from 0x310000 to 0x400000, 960 KiB. ${UNTRUSTED}`,
  );
  assert.equal(
    item(m, "label:ota_0").status.reason,
    `The board's table has no ota_0 entry. ota_0 is missing from the board, and the board's table places factory in its expected space. ${UNTRUSTED}`,
  );
  assert.match(item(m, "label:otadata").status.reason, /places nvs in its expected space/);
  assert.equal(
    item(m, "extra:0").status.reason,
    `The board's table has a nvs entry (type 0x01, subtype 0x02) that the expected layout does not have. It overlaps otadata from 0x00a000 to 0x00c000, 8 KiB. ${UNTRUSTED}`,
  );
  assert.deepEqual(item(m, "extra:1").overlaps, []);
  assert.deepEqual(m.findings, [
    "factory differs",
    "nvs is extra",
    "otadata is missing",
    "phy_init is extra",
    "ota_0 is missing",
    "ota_1 is missing",
    "config_0 is missing",
    "config_1 is missing",
    "diag is missing",
    "reserved is missing",
  ]);
  // The running slot is missing from the board, and is still the default selection.
  assert.equal(defaultSelection(m), "label:ota_0");
});

test("the untrusted tier: the other slot and the otadata are not read, and the next boot is not known", () => {
  const m = model({ tier: "untrusted" });
  const ota1 = inspectorView(m, "label:ota_1");
  assert.deepEqual(ota1?.note, {
    icon: null,
    text: "The firmware does not read this region in the untrusted tier, so no download is offered.",
  });
  assert.deepEqual(ota1?.details?.cells, [
    {
      label: "Image",
      runs: [{ text: "Not read. The firmware does not read the other slot in the untrusted tier.", tone: "muted" }],
    },
  ]);
  assert.deepEqual(
    inspectorView(m, "label:ota_0")?.details?.roleBoxes?.map((box) => [box.label, box.text]),
    [
      ["Booted", "Yes"],
      ["Next boot", "Not known"],
      ["Rollback target", "Not known"],
    ],
  );
  assert.equal(
    inspectorView(m, "label:ota_0")?.bootLine,
    "Booted. The otadata is not read in the untrusted tier, so the next boot is not known.",
  );
  assert.deepEqual(inspectorView(m, "label:otadata")?.details?.notes, [
    { text: "The firmware does not read the otadata in the untrusted tier.", tone: "muted" },
  ]);
  // Every region as expected, below the full tier: the consequence still shows.
  const small = model({ tier: "untrusted", flash: { limit: "flash_size" } });
  assert.equal(summaryView(small).status.level, "warn");
  assert.equal(
    summaryView(small).status.text,
    "Everything matches the expected layout. OTA updates are unavailable and settings are not saved because the flash is smaller than the expected layout.",
  );
});

test("the untrusted tier's own bootloader is unknown and counted", () => {
  const m = model({ tier: "untrusted", flash: { bootloader: internalFlash("untrusted").bootloader } });
  assert.equal(item(m, "bootloader").status.word, "Unknown");
  assert.match(summaryView(m).status.text, /^1 item is not as expected: Bootloader\./);
});

test("the data tier states its limit", () => {
  const m = model({ tier: "data", flash: { limit: "app_descriptor" } });
  assert.equal(
    summaryView(m).status.text,
    "Everything matches the expected layout. OTA updates are unavailable because the running app's descriptor cannot be read.",
  );
});

test("values from a newer firmware show as the API names them", () => {
  const list = layoutRegions();
  const future = {
    ...list[3]!,
    region: "scratchpad" as RegionInfo["region"],
    label: "scratchpad",
    contents: "secret_sauce" as RegionInfo["contents"],
    roles: ["observer" as RegionInfo["roles"][number]],
  };
  const flash: InternalFlash = {
    ...storageInput().flash,
    tier: "partial" as InternalFlash["tier"],
    limit: "gremlins" as InternalFlash["limit"],
    bootloader: { ...storageInput().flash.bootloader, status: "weird" as InternalFlash["bootloader"]["status"] },
  };
  const m = buildModel({ ...storageInput({ entries: editEntries({ otadata: { label: "scratchpad" } }) }), flash, regions: [...list.slice(0, 3), future, ...list.slice(4)] });
  const it = item(m, "label:scratchpad");
  assert.equal(it.code, "??");
  assert.equal(it.subtitle, "Unknown region");
  assert.equal(it.contents, "unknown");
  assert.equal(it.texture, "public");
  assert.deepEqual(it.roles, [{ role: "other", label: "observer" }]);
  assert.equal(inspectorView(m, it.id)?.note?.icon, "lock");
  assert.equal(item(m, "bootloader").status.word, "Unknown");
  assert.equal(m.consequence, "The firmware limits what it does with the flash.");
  assert.equal(limitClause("gremlins"), "the firmware limits what it does with the flash");
  assert.deepEqual(regionIdentity("app_current", "ota_7"), {
    code: "??",
    colour: "--r-rs",
    title: "ota_7",
    subtitle: "Unknown region",
    explanation: "A region this page does not know.",
  });
});

test("tiers and limits make the consequence sentence", () => {
  const cases: [string, string | null, string][] = [
    ["full", null, ""],
    ["data", "bootloader_unlisted", "OTA updates are unavailable because the bootloader is not in the firmware's list of known bootloaders."],
    ["data", null, "OTA updates are unavailable because the firmware limits what it does with the flash."],
    ["untrusted", "table", UNTRUSTED],
    ["untrusted", "flash_size", "OTA updates are unavailable and settings are not saved because the flash is smaller than the expected layout."],
    ["untrusted", "other_table", "OTA updates are unavailable and settings are not saved because the bootloader is not in the firmware's list of known bootloaders, and another partition table on the flash could be the one it read."],
    ["untrusted", "bootloader_invalid", "OTA updates are unavailable and settings are not saved because the bootloader image fails verification."],
    ["untrusted", "app_descriptor", "OTA updates are unavailable and settings are not saved because the running app's descriptor cannot be read."],
    ["other", "table", "The firmware limits what it does with the flash."],
  ];
  for (const [tier, limit, sentence] of cases) assert.equal(consequence(tier, limit), sentence, `${tier} ${limit}`);
});

function updateFlash(state: "new" | "undefined", running: "valid" | "pending_verify" = "valid"): Partial<InternalFlash> {
  return {
    boot: { running: "ota_0", selected: "ota_1", state: running, rollback_target: false, needs_confirmation: running === "pending_verify" },
    otadata: [
      { sector: 0, validity: "valid", seq: 3, state: running, selects: "ota_0" },
      { sector: 1, validity: "valid", seq: 4, state, selects: "ota_1" },
    ],
  };
}

function updateRegions(): RegionInfo[] {
  return layoutRegions().map((info) =>
    info.region === "app_current" ? { ...info, roles: ["running"] } : info.region === "app_alternate" ? { ...info, roles: ["selected"] } : info,
  );
}

test("during an activated update the running slot is the derived rollback target", () => {
  const m = model({ flash: updateFlash("new"), regions: updateRegions() });
  assert.equal(m.boot.next, "update");
  assert.deepEqual(item(m, "label:ota_0").roles, [
    { role: "running", label: "Booted" },
    { role: "rollback_target", label: "Rollback target", derived: true },
  ]);
  assert.deepEqual(item(m, "label:ota_1").roles, [{ role: "selected", label: "Next boot", variant: "update" }]);
  assert.equal(
    summaryView(m).status.text,
    "Everything matches the expected layout. An OTA update in ota_1 is selected for the next boot.",
  );
  assert.equal(
    inspectorView(m, "label:ota_0")?.bootLine,
    "Booted. The next boot starts ota_1. The bootloader rolls back to this slot if the app in the other slot fails to confirm itself.",
  );
  assert.equal(
    inspectorView(m, "label:ota_1")?.bootLine,
    "Selected for the next boot (state new). The slot has not been booted. After booting, the app must confirm itself, otherwise the bootloader rolls back to ota_0.",
  );
  assert.deepEqual(
    inspectorView(m, "label:ota_0")?.details?.roleBoxes?.map((box) => [box.label, box.value]),
    [
      ["Booted", "yes"],
      ["Next boot", "no"],
      ["Rollback target", "yes"],
    ],
  );
});

test("no rollback target is derived while the running app is unconfirmed", () => {
  const m = model({ flash: updateFlash("new", "pending_verify"), regions: updateRegions() });
  assert.equal(m.boot.derivedRollback, false);
  assert.deepEqual(item(m, "label:ota_0").roles, [{ role: "running", label: "Booted" }]);
});

test("a fallback: the plain next-boot chip with its warning, line 4 of the summary and the fallback boot lines", () => {
  const m = model({ flash: updateFlash("undefined"), regions: updateRegions() });
  assert.equal(m.boot.next, "fallback");
  assert.equal(m.boot.derivedRollback, false);
  assert.deepEqual(item(m, "label:ota_1").roles, [
    {
      role: "selected",
      label: "Next boot",
      variant: "fallback",
      tooltip: "The bootloader failed to load this slot at the last boot.",
    },
  ]);
  const summary = summaryView(m);
  assert.equal(summary.status.text, "Everything matches the expected layout.");
  assert.deepEqual(summary.lines[0], {
    level: "warn",
    text: "The otadata selects ota_1, but the bootloader failed to load it and started ota_0.",
  });
  assert.equal(
    inspectorView(m, "label:ota_0")?.bootLine,
    "Booted as a fallback. The otadata selects ota_1, which the bootloader failed to load.",
  );
  assert.equal(
    inspectorView(m, "label:ota_1")?.bootLine,
    "Selected for the next boot by the otadata. The bootloader failed to load this slot at the last boot and started ota_0 instead.",
  );
});

test("boot lines of the running slot: confirmed, awaiting confirmation, nothing selected", () => {
  assert.equal(
    inspectorView(model(), "label:ota_0")?.bootLine,
    "Booted and selected for the next boot. OTA state valid: the app is confirmed, so the bootloader does not roll back.",
  );
  const pending = model({
    flash: {
      boot: { running: "ota_0", selected: "ota_0", state: "pending_verify", rollback_target: true, needs_confirmation: true },
    },
    regions: layoutRegions().map((info) => (info.region === "app_alternate" ? { ...info, roles: ["rollback_target"] } : info)),
  });
  assert.equal(
    inspectorView(pending, "label:ota_0")?.bootLine,
    "Booted. OTA state pending_verify: the app has not confirmed itself, so the next boot rolls back to ota_1.",
  );
  assert.equal(
    inspectorView(pending, "label:ota_1")?.bootLine,
    "Not running. The app in ota_0 has not confirmed itself, so the next boot rolls back to this slot.",
  );
  assert.equal(inspectorView(model(), "label:ota_1")?.bootLine, "Not selected for booting.");
});

test("an unconfirmed running app: the next boot is the rollback target, not the running slot", () => {
  const m = model({
    flash: { boot: { running: "ota_1", selected: "ota_1", state: "pending_verify", rollback_target: true, needs_confirmation: true } },
    regions: layoutRegions().map((info) =>
      info.region === "app_current"
        ? { ...info, roles: ["rollback_target"] }
        : info.region === "app_alternate"
          ? { ...info, roles: ["running", "selected"] }
          : info,
    ),
  });
  assert.deepEqual(item(m, "label:ota_1").roles, [{ role: "running", label: "Booted" }]);
  assert.deepEqual(item(m, "label:ota_0").roles, [
    { role: "selected", label: "Next boot", variant: "fallback", tooltip: "The running app has not confirmed itself, so a restart rolls back to this slot." },
    { role: "rollback_target", label: "Rollback target", derived: false },
  ]);
  assert.equal(
    inspectorView(m, "label:ota_1")?.bootLine,
    "Booted. OTA state pending_verify: the app has not confirmed itself, so the next boot rolls back to ota_0.",
  );
  assert.equal(
    inspectorView(m, "label:ota_0")?.bootLine,
    "Not running. The app in ota_1 has not confirmed itself, so the next boot rolls back to this slot.",
  );
});

test("selection defaults to the running app's region and is kept by key across refreshes", () => {
  const m = model();
  assert.equal(defaultSelection(m), "label:ota_0");
  assert.equal(keepSelection(m, "label:diag"), "label:diag");
  assert.equal(keepSelection(m, "extra:3"), "label:ota_0");
  assert.equal(keepSelection(m, undefined), "label:ota_0");
});

test("the summary names images that fail verification, and a chip size that isn't known", () => {
  const base = storageInput();
  const apps = base.flash.apps.map((app) =>
    app.region === "app_alternate" ? { ...app, image: { ...app.image, status: "invalid" as const, error: "segment_length" as const } } : app,
  );
  const m = buildModel({ ...base, flash: { ...base.flash, apps, capacity: 0 } });
  const summary = summaryView(m);
  assert.equal(summary.title, "8 MiB flash (expected size; the chip's size is not known)");
  assert.deepEqual(summary.lines[0], {
    level: "bad",
    text: "The image in ota_1 fails verification (A segment's length is not a multiple of 4).",
  });
});
