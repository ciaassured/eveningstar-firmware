import assert from "node:assert/strict";
import { test } from "node:test";
import { TABLE_MD5, appImage, editEntries, internalFlash, layoutRegions, storageInput } from "../testing/storage.ts";
import type { ScenarioOptions } from "../testing/storage.ts";
import { STRIP, helpTopic } from "./copy.ts";
import { buildModel, findItem } from "./items.ts";
import type { StorageModel } from "./items.ts";
import { inspectorView, rulerTicks } from "./inspector.ts";
import type { InspectorView } from "./inspector.ts";
import { planDownload } from "./plan.ts";
import { bootloaderStatus, gapStatus, partitionStatus, tableFindings, tableStatus } from "./status.ts";
import { readout, readoutEnds, stripWeights, usagePart } from "./strip.ts";
import { downloadSubject } from "./tray.ts";
import type { AppSlot, InternalFlash } from "./types.ts";

const DEVICE = "eveningstar-0kd7-9c6";

function model(options: ScenarioOptions = {}): StorageModel {
  return buildModel(storageInput(options));
}

function view(m: StorageModel, id: string): InspectorView {
  const found = inspectorView(m, id);
  assert.ok(found, `no item ${id}`);
  return found;
}

function withApp(region: AppSlot["region"], image: Partial<AppSlot["image"]>): Partial<InternalFlash> {
  const flash = internalFlash();
  return { apps: flash.apps.map((app) => (app.region === region ? { ...app, image: { ...app.image, ...image } } : app)) };
}

test("the download buttons of each region, with their labels and sublabels", () => {
  const m = model();
  const actions = (id: string) => view(m, id).actions.map((a) => [a.key, a.title, a.sublabel, a.primary, a.disabled]);
  assert.deepEqual(actions("label:ota_0"), [
    ["image", "Download image", "4 KiB, SHA-256 appended inside the file", true, false],
    ["whole", "Whole region", "3 MiB, raw bytes", false, false],
  ]);
  assert.deepEqual(view(m, "label:ota_0").actions.map((a) => a.target), [
    { region: "app_current", what: { kind: "image" } },
    { region: "app_current", what: { kind: "whole" } },
  ]);
  assert.deepEqual(actions("label:ota_1"), [
    ["image", "Download image", "No valid image in this slot", true, true],
    ["whole", "Whole region", "3 MiB, raw bytes", false, false],
  ]);
  assert.deepEqual(actions("bootloader"), [
    ["image", "Download bootloader", "22 KiB, SHA-256 appended inside the file", true, false],
    ["whole", "Whole region", "32 KiB, raw bytes", false, false],
  ]);
  assert.deepEqual(actions("partition_table"), [
    ["table", "Download table", "3 KiB (0xc00): entries, MD5 entry and padding in one file", true, false],
  ]);
  assert.deepEqual(view(m, "partition_table").actions[0]?.target, { region: "partition_table", what: { kind: "whole" } });
  assert.deepEqual(actions("label:otadata"), [["otadata", "Download otadata", "8 KiB, both records", true, false]]);

  const checking = model({ flash: withApp("app_current", { status: "checking", file_sha256: null, digest: null }) });
  assert.deepEqual(view(checking, "label:ota_0").actions[0]?.sublabel, "The image check has not finished");
  assert.equal(view(checking, "label:ota_0").actions[0]?.disabled, true);
  const invalid = model({ flash: { bootloader: { ...internalFlash().bootloader, status: "invalid", size: null, error: "magic" } } });
  assert.deepEqual(
    view(invalid, "bootloader").actions.map((a) => [a.sublabel, a.disabled]),
    [
      ["The bootloader image does not verify", true],
      ["32 KiB, raw bytes", false],
    ],
  );
  assert.deepEqual(findItem(invalid, "bootloader")?.status, {
    level: "bad",
    word: "Invalid",
    reason: "The bootloader image fails verification (No image header).",
  });
});

test("regions without a download show a note in its place", () => {
  const m = model();
  const note = (id: string) => view(m, id).note;
  assert.deepEqual(note("label:factory"), {
    icon: "lock",
    text: "Raw region downloads are disabled because the region potentially contains secrets, such as cryptographic keys.",
  });
  assert.deepEqual(note("label:config_1"), note("label:factory"));
  assert.deepEqual(note("label:reserved"), {
    icon: null,
    text: "Reserved for future firmware. The region is not read, so no download is offered.",
  });
  assert.deepEqual(note("gap:49152"), { icon: null, text: "No data is stored in a gap, so no download is offered." });
  assert.equal(note("label:ota_0"), undefined);
  assert.deepEqual(view(m, "label:factory").actions, []);
});

test("the inspector's header: the pill reads As expected when ok, and the Why popover holds the reason", () => {
  const m = model();
  const ota0 = view(m, "label:ota_0");
  assert.equal(ota0.label, "Inspector: ota_0");
  assert.deepEqual(ota0.pill, { word: "As expected", name: "As expected. Show why" });
  assert.equal(ota0.why.word, "As expected");
  assert.equal(
    ota0.why.note,
    "Whether the partition table on the board has this entry exactly as the expected layout does. If not, the firmware restricts what it does with the flash.",
  );
  assert.equal(
    ota0.explanation,
    "The OTA app slot the device is currently running from. It holds one app image; the remainder of the slot is unused.",
  );
  assert.equal(ota0.expected, undefined);
  assert.equal(view(m, "gap:35840").explanation, "Not covered by any partition table entry. Nothing is stored here.");
  const missing = model({ entries: editEntries({ reserved: null }) });
  assert.deepEqual(view(missing, "label:reserved").pill, { word: "Missing", name: "Missing. Show why" });
});

test("the usage summary of an app slot: the image and the unused rest to scale, with ticks and spans", () => {
  const usage = view(model(), "label:ota_0").usage;
  assert.ok(usage);
  assert.equal(usage.title, "Usage summary");
  assert.equal(usage.tableLabel, "Spans of ota_0");
  assert.deepEqual(
    usage.parts.map((p) => [p.key, p.label, p.short, p.fill, p.offset, p.size, p.labelled, p.caption, p.tooltip]),
    [
      ["image", "Image", "Image", "used", 0x1_0000, 4_096, false, { short: "Image", size: "4 KiB", percent: "0.1% of the region" }, "Image: 0x010000 to 0x011000, 4 KiB"],
      ["unused", "Unused, erased", "Unused", "unused", 0x1_1000, 0x2f_f000, true, { short: "Unused", size: "3 MiB" }, "Unused, erased: 0x011000 to 0x310000, 3 MiB"],
    ],
  );
  assert.deepEqual(
    usage.ticks.map((t) => [t.text, t.row, t.align]),
    [
      ["0x010000", 1, "start"],
      ["0x011000", 2, "center"],
      ["0x310000", 1, "end"],
    ],
  );
  assert.deepEqual(
    usage.rows.map((r) => [r.key, r.label, r.square, r.span.start.hex, r.span.end.hex, r.span.size.text]),
    [
      ["region", "Region", "region", "0x010000", "0x310000", "3,145,728 bytes, 3 MiB"],
      ["image", "Image", "used", "0x010000", "0x011000", "4,096 bytes, 4 KiB"],
      ["unused", "Unused, erased", "unused", "0x011000", "0x310000", "3,141,632 bytes, 3 MiB"],
    ],
  );
  assert.equal(usage.text, undefined);
});

test("a large image is labelled on the ruler with its share of the region", () => {
  const big = appImage(0x12_0000, 9);
  const m = buildModel({ ...storageInput(), flash: internalFlash("full", big) });
  const image = view(m, "label:ota_0").usage?.parts[0];
  assert.equal(image?.labelled, true);
  assert.deepEqual(image?.caption, { short: "Image", size: "1.13 MiB", percent: "37.5% of the region" });
  assert.equal(findItem(m, "label:ota_0")?.usage, 0x12_0000 / 0x30_0000);
});

test("the usage summary of the partition table, the otadata and the regions without parts", () => {
  const m = model();
  assert.deepEqual(
    view(m, "partition_table").usage?.parts.map((p) => [p.label, p.short, p.fill, p.size, p.caption.size]),
    [
      ["8 entries", "Entries", "used", 256, "256 bytes"],
      ["MD5 entry", "MD5", "alternate", 32, "32 bytes"],
      ["Unused, erased", "Unused", "unused", 2_784, "2.72 KiB"],
    ],
  );
  assert.deepEqual(
    view(m, "label:otadata").usage?.parts.map((p) => [p.label, p.short, p.fill, p.offset, p.size, p.labelled]),
    [
      ["Sector 0 record", "Sector 0", "used", 0xa000, 0x1000, true],
      ["Sector 1 record", "Sector 1", "alternate", 0xb000, 0x1000, true],
    ],
  );
  assert.deepEqual(
    view(m, "bootloader").usage?.parts.map((p) => [p.label, p.size]),
    [
      ["Bootloader image", 22_528],
      ["Unused, erased", 0x8000 - 22_528],
    ],
  );
  const text = (id: string) => view(m, id).usage?.text;
  assert.equal(text("label:factory"), "Factory data written during manufacturing, 4 KiB");
  assert.equal(text("label:config_0"), "Saved settings of the running app, 128 KiB");
  assert.equal(text("label:config_1"), "Saved settings of the other app, 128 KiB");
  assert.equal(text("label:diag"), "Diagnostic crash and event records, 256 KiB");
  assert.equal(text("label:reserved"), "Reserved, unused, 1.44 MiB");
  assert.equal(text("gap:35840"), "Nothing stored, 1 KiB");
  assert.equal(text("label:ota_1"), "Erased, no image, 3 MiB");
  assert.equal(view(m, "label:factory").usage?.texture, "sensitive");
  const checking = model({ flash: withApp("app_current", { status: "checking" }) });
  assert.equal(view(checking, "label:ota_0").usage?.text, "Image check running, 3 MiB");
  const invalid = model({ flash: withApp("app_current", { status: "invalid", error: "magic" }) });
  assert.equal(view(invalid, "label:ota_0").usage?.text, "No valid image, 3 MiB");
  assert.equal(findItem(invalid, "label:ota_0")?.usage, undefined);
});

test("ticks too close together go to a second row, and are left out when both rows are crowded, except the last", () => {
  const ticks = (ends: number[]) => rulerTicks(0, 100, ends).map((t) => [t.offset, t.row, t.align]);
  assert.deepEqual(ticks([5, 10, 50]), [
    [0, 1, "start"],
    [5, 2, "center"],
    [50, 1, "center"],
    [100, 1, "end"],
  ]);
  assert.deepEqual(ticks([5, 90, 95]), [
    [0, 1, "start"],
    [5, 2, "center"],
    [90, 1, "center"],
    [95, 2, "center"],
    [100, 2, "end"],
  ]);
  assert.deepEqual(ticks([100, 100]), [
    [0, 1, "start"],
    [100, 1, "end"],
  ]);
});

test("an app slot's details: role boxes, facts and hashes", () => {
  const image = appImage();
  const details = view(model(), "label:ota_0").details;
  assert.ok(details);
  assert.deepEqual(
    details.roleBoxes?.map((b) => [b.label, b.value, b.text, b.icon]),
    [
      ["Booted", "yes", "Yes", "booted"],
      ["Next boot", "yes", "Yes", "next"],
      ["Rollback target", "no", "No", undefined],
    ],
  );
  assert.deepEqual(details.cells, [
    {
      label: "Image",
      icon: "ok",
      runs: [
        { text: "Valid", tone: "ok" },
        { text: " checked 3 s after boot", tone: "muted" },
      ],
    },
    {
      label: "Project and version",
      runs: [
        { text: "eveningstar ", tone: "plain" },
        { text: "0.1.0", tone: "strong" },
      ],
    },
    { label: "Built", runs: [{ text: "Oct 31 2026, 10:12:44", tone: "plain" }] },
    { label: "Built with", runs: [{ text: "esp-hal", tone: "plain" }] },
    { label: "Secure version", runs: [{ text: "0", tone: "plain" }] },
  ]);
  assert.equal(
    details.hashes?.note,
    "The SHA-256 appended in the image is part of it and is verified by the bootloader. The SHA-256 of the image file is the value that file hashing tools print for the downloaded file.",
  );
  assert.deepEqual(details.hashes?.rows, [
    {
      key: "file_sha256",
      label: "SHA-256 of the image file",
      under: "4,096 bytes, including the appended SHA-256",
      value: image.fileSha256,
      name: "SHA-256 of the image file, 4,096 bytes, including the appended SHA-256",
      help: "file_sha256",
      helpLabel: "What is the SHA-256 of the image file?",
    },
    {
      key: "digest",
      label: "SHA-256 appended in image",
      under: "validation hash, over the first 4,064 bytes",
      value: image.digest,
      name: "SHA-256 appended in image, validation hash, over the first 4,064 bytes",
      help: "stored_digest",
      helpLabel: "What is the SHA-256 appended in image?",
    },
    {
      key: "elf_sha256",
      label: "ELF SHA-256",
      under: "of the ELF file the image was built from",
      value: "9a0b".repeat(16),
      name: "ELF SHA-256, of the ELF file the image was built from",
    },
  ]);
});

test("an app slot's image cell for each image state, and the note for a shifted slot", () => {
  const cell = (m: StorageModel, id: string) => view(m, id).details?.cells[0];
  assert.deepEqual(cell(model(), "label:ota_1"), { label: "Image", runs: [{ text: "None. The slot is erased.", tone: "plain" }] });
  assert.deepEqual(cell(model({ flash: withApp("app_current", { status: "checking" }) }), "label:ota_0"), {
    label: "Image",
    runs: [{ text: "Checking. The background check has not finished.", tone: "muted" }],
  });
  const shifted = model({
    flash: withApp("app_alternate", { status: "invalid", error: "magic" }),
    entries: editEntries({ ota_1: { offset: 0x32_0000, size: 0x20_0000 } }),
  });
  const details = view(shifted, "label:ota_1").details;
  assert.deepEqual(details?.cells[0], {
    label: "Image",
    icon: "bad",
    runs: [
      { text: "Not valid", tone: "bad" },
      { text: " (No image header)", tone: "muted" },
    ],
  });
  assert.deepEqual(details?.notes, [
    {
      text: "The data at the expected offset is not an image header. The board's table places ota_1 at a different offset, so the image may be located elsewhere. This does not indicate damage.",
      tone: "note",
    },
  ]);
  assert.equal(details?.hashes, undefined);
});

test("the bootloader's details leave out the ESP-IDF version its name already holds", () => {
  const details = view(model(), "bootloader").details;
  assert.deepEqual(
    details?.cells.map((c) => [c.label, c.icon, c.runs.map((r) => `${r.tone}:${r.text}`).join("|")]),
    [
      ["Status", "ok", "ok:Expected"],
      ["Known as", undefined, "mono:esp-idf-v5.5.1-rollback"],
      ["Bootloader version", undefined, "plain:1"],
      ["Built", undefined, "plain:Oct 30 2026 12:00:00"],
    ],
  );
  assert.deepEqual(
    details?.hashes?.rows.map((r) => [r.key, r.under]),
    [
      ["file_sha256", "22,528 bytes, including the appended SHA-256"],
      ["digest", "validation hash, over the first 22,496 bytes"],
    ],
  );
  assert.match(details?.hashes?.note ?? "", /verified by the ROM/);
  const unlisted = model({ flash: { bootloader: internalFlash("data").bootloader } });
  assert.deepEqual(
    view(unlisted, "bootloader").details?.cells.slice(0, 3).map((c) => [c.label, c.runs.map((r) => r.text).join("")]),
    [
      ["Status", "Unknown"],
      ["Known as", "Not in the firmware's list"],
      ["ESP-IDF", "v5.5.1"],
    ],
  );
});

test("the partition table's details: entries, MD5 check, where it was read, and the stored MD5", () => {
  const details = view(model(), "partition_table").details;
  assert.deepEqual(
    details?.cells.map((c) => [c.label, c.icon, c.runs.map((r) => r.text).join("")]),
    [
      ["Entries", undefined, "8, matching the expected layout"],
      ["MD5 check", "ok", "Matches stored MD5 against the entries"],
      ["Read from", undefined, "0x008000 32,768"],
    ],
  );
  assert.deepEqual(details?.hashes, {
    title: "Hashes",
    note: "The table download includes this MD5. The device does not hash the downloaded file as a whole.",
    rows: [
      {
        key: "md5",
        label: "Stored MD5",
        under: "over the 8 entries, kept in the MD5 entry",
        value: TABLE_MD5,
        name: "Stored MD5, over the 8 entries, kept in the MD5 entry",
        help: "md5",
        helpLabel: "What is the Stored MD5?",
      },
    ],
  });
  const differing = model({ entries: editEntries({ diag: null }), read: { truncated: true, md5: null, end: "erased" } });
  const cells = view(differing, "partition_table").details?.cells.map((c) => c.runs.map((r) => r.text).join(""));
  assert.deepEqual(cells?.slice(0, 2), ["7, differing from the expected layout", "No MD5 entry"]);
  assert.deepEqual(view(differing, "partition_table").details?.notes, [
    { text: "The table has more entries than the device lists (7).", tone: "note" },
  ]);
  assert.equal(view(differing, "partition_table").details?.hashes, undefined);
  assert.equal(
    findItem(differing, "partition_table")?.status.reason,
    "The table has no MD5 entry, and it differs from the expected layout: diag is missing.",
  );
});

test("the otadata's details: each record's CRC, the record that boots, and what its states mean", () => {
  const details = view(model(), "label:otadata").details;
  assert.deepEqual(details?.otadata?.columns, ["Sector", "CRC", "Seq", "State", "Selects"]);
  assert.deepEqual(details?.otadata?.rows, [
    { sector: "0", crc: { text: "CRC match", match: true }, seq: "3", state: "valid", selects: "ota_0", boots: true },
    { sector: "1", crc: { text: "Blank, erased", match: false }, seq: "–", state: "–", selects: "–", boots: false },
  ]);
  assert.deepEqual(details?.otadata?.states, [
    {
      state: "valid",
      text: "Confirmed. The app has booted and marked itself valid. The bootloader does not roll back from a confirmed app.",
    },
  ]);
  assert.equal(
    details?.otadata?.rule,
    "The bootloader boots the slot named by the record with a matching CRC and the highest sequence number (marked next boot).",
  );
  assert.deepEqual(details?.hashes?.rows, []);
  const corrupt = model({
    flash: {
      otadata: [
        { sector: 0, validity: "corrupt", seq: 9, state: "new", selects: "ota_1" },
        { sector: 1, validity: "maybe" as "valid", seq: null, state: "sideways" as "new", selects: null },
      ],
    },
  });
  const rows = view(corrupt, "label:otadata").details?.otadata;
  assert.deepEqual(
    rows?.rows.map((r) => [r.crc.text, r.boots]),
    [
      ["CRC mismatch", false],
      ["maybe", false],
    ],
  );
  assert.deepEqual(rows?.states.map((s) => s.text), [
    "Written by an OTA update and selected for the next boot. The app has not been booted.",
    "A state this page does not know.",
  ]);
});

test("sensitive and reserved regions and gaps have no details", () => {
  const m = model();
  for (const id of ["label:factory", "label:config_0", "label:diag", "label:reserved", "gap:49152"]) {
    assert.equal(view(m, id).details, undefined, id);
  }
});

test("the strip's widths: a minimum share each, the rest by size, always filling the strip", () => {
  const sizes = [0x8000, 0x1000, 0x3_0000, 0x30_0000, 0x4c_7000];
  const total = sizes.reduce((a, b) => a + b, 0);
  const few = stripWeights(sizes, total);
  assert.equal(few.minimum, 6);
  assert.ok(Math.abs(few.share.reduce((a, b) => a + b, 0) - 1) < 1e-12);
  assert.ok(few.share.every((share) => share >= 0.06 - 1e-12));
  assert.ok(Math.abs((few.grow[0] ?? 0) - (6 + (70 * 0x8000) / total)) < 1e-12);

  const many = stripWeights(Array.from({ length: 20 }, () => total / 20), total);
  assert.equal(many.minimum, 3);
  assert.ok(many.share.every((share) => share >= 0.03 - 1e-12));
  assert.equal(stripWeights(Array.from({ length: 10 }, () => 1), 10).minimum, 6);
  assert.equal(stripWeights(Array.from({ length: 11 }, () => 1), 11).minimum, 60 / 11);

  const overlapping = stripWeights([total, total / 2], total);
  assert.ok(Math.abs(overlapping.share.reduce((a, b) => a + b, 0) - 1) < 1e-12);
  assert.ok((overlapping.share[0] ?? 0) > (overlapping.share[1] ?? 0));
  assert.deepEqual(stripWeights([], 1), { grow: [], share: [], minimum: 6 });

  const m = model();
  assert.equal(m.minimum, 5);
  assert.ok(Math.abs(m.items.reduce((sum, it) => sum + it.share, 0) - 1) < 1e-12);
  assert.ok(m.items.every((it) => it.grow >= 5));
});

test("the readout names a region with its range, and the strip's ends", () => {
  const ota0 = findItem(model(), "label:ota_0");
  assert.ok(ota0);
  assert.deepEqual(readout(ota0), {
    title: "ota_0",
    subtitle: "Running app",
    start: "0x010000",
    end: "0x310000",
    size: "3 MiB",
  });
  assert.deepEqual(readoutEnds(0x80_0000), { start: "0x000000", end: "0x800000, 8 MiB" });
  assert.equal(STRIP.label(0x80_0000), "Flash map, 8 MiB, in flash order");
  assert.match(helpTopic("strip", 0x80_0000).text, /to the end of the 8 MiB flash\./);
});

test("a usage bar on the strip stands for the first part of its region's ruler", () => {
  const m = model();
  const withUsage = m.items.filter((it) => it.usage !== undefined);
  assert.deepEqual(
    withUsage.map((it) => it.id),
    ["bootloader", "partition_table", "label:otadata", "label:ota_0"],
  );
  for (const it of withUsage) {
    const part = usagePart(it.info?.region ?? "");
    assert.equal(part, inspectorView(m, it.id)?.usage?.parts[0]?.key, it.id);
  }
  assert.equal(usagePart("factory"), undefined);
});

test("statuses and their reasons, case by case", () => {
  const flash = internalFlash();
  const cases: [string, ReturnType<typeof gapStatus>, ReturnType<typeof gapStatus>][] = [
    [
      "bootloader unlisted",
      bootloaderStatus({ ...flash.bootloader, status: "unlisted" }),
      {
        level: "warn",
        word: "Unknown",
        reason:
          "The bootloader image verifies, but the bootloader is not in the firmware's list of known bootloaders. Rollback support cannot be assumed. OTA updates are unavailable.",
      },
    ],
    [
      "bootloader invalid",
      bootloaderStatus({ ...flash.bootloader, status: "invalid", error: "checksum" }),
      { level: "bad", word: "Invalid", reason: "The bootloader image fails verification (The checksum does not match)." },
    ],
    [
      "bootloader overlapped",
      bootloaderStatus(flash.bootloader, [{ title: "scratch", subtitle: "Extra entry", start: 0, end: 0x1000 }], ""),
      {
        level: "warn",
        word: "Overlap",
        reason:
          "The bootloader (esp-idf-v5.5.1-rollback) is in the firmware's list of known bootloaders and supports rollback. It overlaps scratch from 0x000000 to 0x001000, 4 KiB.",
      },
    ],
    [
      "bootloader from a newer firmware",
      bootloaderStatus({ ...flash.bootloader, status: "patched" as "known" }),
      { level: "warn", word: "Unknown", reason: "The device reports the bootloader status patched, which this page does not know." },
    ],
    [
      "table without MD5 entry",
      tableStatus({ comparison: "compare", md5: null, findings: [], search: "", consequence: "" }),
      { level: "ok", word: "Expected", reason: "Every entry matches the expected layout. The table has no MD5 entry." },
    ],
    [
      "table not compared, no slot",
      tableStatus({ comparison: "no-slot", md5: { stored: TABLE_MD5, matches: true }, findings: [], search: "", consequence: "" }),
      {
        level: "warn",
        word: "Unknown",
        reason: "The app did not boot from one of the expected layout's OTA app slots, so the board's table is not compared.",
      },
    ],
    [
      "table with a damaged MD5 and no comparison",
      tableStatus({ comparison: "unknown", md5: { stored: TABLE_MD5, matches: false }, findings: [], search: "", consequence: "" }),
      { level: "bad", word: "MD5 differs", reason: "The stored MD5 does not match the partition table entries. The table may be damaged." },
    ],
    [
      "partition differing in several fields",
      partitionStatus({ comparison: "compare", label: "ota_1", onBoard: true, differs: ["type", "offset", "size"], overlaps: [], consequence: "" }),
      { level: "warn", word: "Differs", reason: "The ota_1 entry in the board's table differs from the expected layout in type, offset and size." },
    ],
    [
      "partition overlapped twice",
      partitionStatus({
        comparison: "compare",
        label: "ota_0",
        onBoard: true,
        differs: [],
        overlaps: [
          { title: "a", subtitle: "Extra entry", start: 0x1_0000, end: 0x1_1000 },
          { title: "b", subtitle: "Extra entry", start: 0x2_0000, end: 0x2_2000 },
        ],
        consequence: "",
      }),
      {
        level: "warn",
        word: "Overlap",
        reason:
          "The ota_0 entry in the board's table matches the expected layout, but a overlaps it from 0x010000 to 0x011000, 4 KiB, and b overlaps it from 0x020000 to 0x022000, 8 KiB.",
      },
    ],
    [
      "gap used by two entries",
      gapStatus("compare", ["ota_0", "ota_1"]),
      { level: "warn", word: "Used", reason: "The board's table places ota_0 and ota_1 in this space." },
    ],
    [
      "gap with no slot",
      gapStatus("no-slot", []),
      {
        level: "warn",
        word: "Unknown",
        reason: "The app did not boot from one of the expected layout's OTA app slots, so the board's table is not compared.",
      },
    ],
  ];
  for (const [name, actual, expected] of cases) assert.deepEqual(actual, expected, name);
});

test("table findings list the items in order, then the order, the offset and the end", () => {
  const subject = (title: string, kind: "region" | "missing" | "extra", extra: { differs?: ["size"]; overlaps?: { title: string }[] } = {}) => ({
    kind,
    title,
    boot: false,
    differs: extra.differs ?? [],
    overlaps: extra.overlaps ?? [],
  });
  assert.deepEqual(
    tableFindings({
      items: [
        { kind: "region", title: "Partition table", boot: true, differs: [], overlaps: [{ title: "x" }] },
        subject("ota_0", "region", { overlaps: [{ title: "x" }, { title: "y" }] }),
        subject("ota_1", "missing"),
        subject("diag", "region", { differs: ["size"] }),
        subject("x", "extra"),
      ],
      orderDiffers: true,
      firmwareMatches: false,
      readAt: 0x6000,
      expectedAt: 0x8000,
      malformed: true,
    }),
    [
      "ota_0 overlaps x and y",
      "ota_1 is missing",
      "diag differs",
      "x is extra",
      "the entries are in a different order than in the expected layout",
      "the bootloader read the table at 0x006000, not at 0x008000",
      "the table ends at an entry that is not valid",
    ],
  );
  assert.deepEqual(
    tableFindings({ items: [], orderDiffers: false, firmwareMatches: false, readAt: 0x6000, expectedAt: 0x8000, malformed: false }),
    ["the bootloader read the table at 0x006000, not at 0x008000"],
  );
});

test("the table download carries the stored MD5 of the table the region holds", () => {
  const input = storageInput();
  const facts = { flash: input.flash, regions: input.regions, device: DEVICE, table: input.table };
  const table = planDownload({ region: "partition_table", what: { kind: "whole" } }, facts);
  assert.ok(table.ok);
  assert.equal(table.plan.md5, TABLE_MD5);
  assert.equal(table.plan.size, 0xc00);
  const otadata = planDownload({ region: "otadata", what: { kind: "whole" } }, facts);
  assert.ok(otadata.ok);
  assert.equal(otadata.plan.md5, undefined);

  // In the untrusted tier the region is the board's table, wherever it is.
  const other = "0123456789abcdef".repeat(2);
  const moved = storageInput({ tier: "untrusted", boardOffset: 0x6000, read: { md5: { stored: other, matches: true } } });
  const movedFacts = { flash: moved.flash, regions: moved.regions, device: DEVICE };
  const fromBoard = planDownload({ region: "partition_table", what: { kind: "whole" } }, { ...movedFacts, table: moved.table });
  assert.ok(fromBoard.ok);
  assert.equal(fromBoard.plan.md5, other);
  const unknown = planDownload({ region: "partition_table", what: { kind: "whole" } }, movedFacts);
  assert.ok(unknown.ok);
  assert.equal(unknown.plan.md5, undefined);
});

test("what each download is in the tray, with its hash and note", () => {
  const input = storageInput();
  const facts = { flash: input.flash, regions: input.regions, device: DEVICE, table: input.table };
  const plan = (target: Parameters<typeof planDownload>[0]) => {
    const result = planDownload(target, facts);
    assert.ok(result.ok);
    return result.plan;
  };
  const image = { region: "app_current", what: { kind: "image" } } as const;
  assert.deepEqual(downloadSubject(image, input.regions, plan(image)), {
    title: "ota_0",
    subtitle: "Running app",
    what: "App image, ota_0",
    hash: { label: "SHA-256 of the image file", text: "the value for the file you receive", value: appImage().fileSha256 },
  });
  assert.deepEqual(downloadSubject(image, input.regions), { title: "ota_0", subtitle: "Running app", what: "App image, ota_0" });
  const bootloader = { region: "bootloader", what: { kind: "image" } } as const;
  assert.equal(downloadSubject(bootloader, input.regions, plan(bootloader)).what, "Bootloader image");
  const table = { region: "partition_table", what: { kind: "whole" } } as const;
  assert.deepEqual(downloadSubject(table, input.regions, plan(table)), {
    title: "Partition table",
    subtitle: "Map of the flash",
    what: "Partition table, entries and MD5 entry",
    note: "The device does not hash the file as a whole.",
    hash: { label: "Stored MD5", text: "over the entries; the MD5 entry is part of the file", value: TABLE_MD5 },
  });
  const otadata = { region: "otadata", what: { kind: "whole" } } as const;
  assert.deepEqual(downloadSubject(otadata, input.regions, plan(otadata)), {
    title: "otadata",
    subtitle: "Boot selection",
    what: "Boot selection (otadata)",
    note: "No hash: the device does not hash whole regions.",
  });
  const whole = { region: "app_current", what: { kind: "whole" } } as const;
  assert.deepEqual(downloadSubject(whole, input.regions, plan(whole)), {
    title: "ota_0",
    subtitle: "Running app",
    what: "Whole region, ota_0",
    note: "No hash: the device does not hash whole regions.",
  });
});

test("the layout fixture matches the firmware: the table region is 0xc00 bytes and app_current is always running", () => {
  for (const tier of ["full", "untrusted"] as const) {
    const list = layoutRegions({ tier });
    assert.deepEqual(list.find((info) => info.region === "partition_table")?.expected, { offset: 0x8000, size: 0xc00 });
    assert.ok(list.find((info) => info.region === "app_current")?.roles.includes("running"));
  }
});
