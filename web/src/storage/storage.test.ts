import assert from "node:assert/strict";
import { test } from "node:test";
import { appImage, boardTable, internalFlash, regions } from "../testing/storage.ts";
import { downloadFilename, unverifiedFilename } from "./filenames.ts";
import { compareEntries, flashSummary, healthNotices, layoutEntries } from "./health.ts";
import { isReadOnly, lockReason, matchLabel, subtypeName, typeName } from "./labels.ts";
import { barShares, flashTotal, mapSegments, MIN_SHARE, withGaps } from "./map.ts";
import { planDownload } from "./plan.ts";
import { parseOffset, parseSpan } from "./range.ts";
import { bootChain, downloadChoices, firmwareView, regionRow } from "./view.ts";

const DEVICE = "eveningstar-0kd7-9c6";

test("partition types and subtypes have their ESP-IDF names, and flag bit 1 is read-only", () => {
  assert.equal(typeName(0), "app");
  assert.equal(typeName(1), "data");
  assert.equal(typeName(0x40), "0x40");
  assert.equal(subtypeName(0, 0x00), "factory");
  assert.equal(subtypeName(0, 0x10), "ota_0");
  assert.equal(subtypeName(0, 0x1f), "ota_15");
  assert.equal(subtypeName(0, 0x20), "test");
  assert.equal(subtypeName(1, 0x00), "ota");
  assert.equal(subtypeName(1, 0x02), "nvs");
  assert.equal(subtypeName(1, 0x06), "undefined");
  assert.equal(subtypeName(1, 0x83), "littlefs");
  assert.equal(subtypeName(1, 0x99), "0x99");
  assert.equal(isReadOnly(2), true);
  assert.equal(isReadOnly(1), false);
});

test("a layout that differs names its fields", () => {
  assert.deepEqual(matchLabel("differs", ["offset", "size"]), { level: "caution", label: "Differs in offset and size" });
  assert.deepEqual(matchLabel("missing", []), { level: "serious", label: "Missing" });
  assert.equal(matchLabel("from_the_future", []).label, '"from_the_future"');
});

test("each region says why it can't be downloaded", () => {
  const [bootloader, , factory, , current, alternate, , , , reserved] = regions();
  assert.ok(bootloader && factory && current && alternate && reserved);
  assert.equal(lockReason(bootloader, "full"), undefined);
  assert.match(lockReason(factory, "full") ?? "", /never leaves the device/);
  assert.equal(lockReason(reserved, "full"), "Reserved for later firmware");
  const [, , , , , untrustedAlternate] = regions({ tier: "untrusted" });
  assert.ok(untrustedAlternate);
  assert.equal(lockReason(untrustedAlternate, "untrusted"), "Not readable in the untrusted tier");
});

test("gaps are the space between regions and after the last", () => {
  const list = regions();
  const gaps = (total: number) =>
    withGaps(list, total)
      .filter((area) => area.kind === "gap")
      .map((area) => [area.offset, area.size]);
  assert.deepEqual(gaps(0x80_0000), [[0xc000, 0x4000]]);
  assert.deepEqual(gaps(0x100_0000), [
    [0xc000, 0x4000],
    [0x80_0000, 0x80_0000],
  ]);
  assert.equal(flashTotal(0, 0x80_0000, list), 0x80_0000);
  assert.equal(flashTotal(0x100_0000, 0x80_0000, list), 0x100_0000);
});

test("the map's shares are proportional with a floor, and add up to the whole bar", () => {
  const shares = barShares([0x8000, 0x1000, 0x30_0000, 0x30_0000, 0x1_0000], MIN_SHARE);
  assert.ok(Math.abs(shares.reduce((a, b) => a + b, 0) - 1) < 1e-9);
  assert.ok(shares.every((share) => share >= MIN_SHARE - 1e-12));
  assert.ok(Math.abs((shares[2] ?? 0) - (shares[3] ?? 0)) < 1e-12);
  assert.deepEqual(barShares([1, 1, 1], 0.5), [1 / 3, 1 / 3, 1 / 3]);
  assert.deepEqual(barShares([], 0.1), []);
});

test("the map marks the running slot, the image fill, and labels only wide segments", () => {
  const list = regions();
  const flash = internalFlash();
  const segments = mapSegments(withGaps(list, 0x80_0000), flash.apps);
  const current = segments.find((segment) => segment.region === "app_current");
  assert.equal(current?.current, true);
  assert.equal(current?.labelled, true);
  assert.ok(Math.abs((current?.fill ?? 0) - 4_096 / 0x30_0000) < 1e-12);
  assert.equal(segments.find((segment) => segment.region === "app_alternate")?.fill, undefined);
  assert.equal(segments.find((segment) => segment.region === "factory")?.labelled, false);
  assert.equal(segments.find((segment) => segment.key === "gap-49152")?.name, "unallocated");
});

test("filenames follow the device's names", () => {
  const image = { length: 4_096, fileSha256: "07aa31c2".padEnd(64, "0") };
  const subject = { device: DEVICE, region: "app_current", slot: "ota_0", regionSize: 0x30_0000, image } as const;
  assert.equal(downloadFilename({ ...subject, span: { first: 0, last: 4_095 } }), `${DEVICE}-app_current-ota_0-image-07aa31c2.bin`);
  assert.equal(downloadFilename(subject), `${DEVICE}-app_current-ota_0-slot-of-07aa31c2.bin`);
  assert.equal(downloadFilename({ ...subject, span: { first: 0, last: 0xfff } , image: { length: 8_192, fileSha256: image.fileSha256 } }), `${DEVICE}-app_current-ota_0-0x0-0xfff.bin`);
  assert.equal(
    downloadFilename({ ...subject, image: { length: 4_096, fileSha256: null } }),
    `${DEVICE}-app_current-ota_0-0x0-0x2fffff.bin`,
  );
  assert.equal(
    downloadFilename({ device: DEVICE, region: "bootloader", regionSize: 0x8000, span: { first: 0, last: 22_527 }, image: { length: 22_528, fileSha256: "8d02b7e4".padEnd(64, "1") } }),
    `${DEVICE}-bootloader-image-8d02b7e4.bin`,
  );
  assert.equal(downloadFilename({ device: DEVICE, region: "bootloader", regionSize: 0x8000 }), `${DEVICE}-bootloader.bin`);
  assert.equal(downloadFilename({ device: DEVICE, region: "app_current", slot: null, regionSize: 0x30_0000, span: { first: 16, last: 31 } }), `${DEVICE}-app_current-0x10-0x1f.bin`);
  assert.equal(downloadFilename({ device: "bad name/../x", region: "otadata", regionSize: 0x2000 }), "bad-name-..-x-otadata.bin");
  assert.equal(unverifiedFilename(`${DEVICE}-bootloader.bin`), `${DEVICE}-bootloader-UNVERIFIED.bin`);
});

test("an image download is planned against its file_sha256, and a whole slot against the image's digest", () => {
  const image = appImage();
  const flash = internalFlash("full", image);
  const facts = { flash, regions: regions(), device: DEVICE };
  const alone = planDownload({ region: "app_current", what: { kind: "image" } }, facts);
  assert.ok(alone.ok);
  assert.deepEqual(alone.plan.span, { first: 0, last: 4_095 });
  assert.equal(alone.plan.size, 4_096);
  assert.deepEqual(alone.plan.check, { sha256: image.fileSha256, bytes: 4_096, against: "file_sha256" });
  assert.equal(alone.plan.filename, `${DEVICE}-app_current-ota_0-image-${image.fileSha256.slice(0, 8)}.bin`);

  const whole = planDownload({ region: "app_current", what: { kind: "whole" } }, facts);
  assert.ok(whole.ok);
  assert.equal(whole.plan.span, undefined);
  assert.equal(whole.plan.size, 0x30_0000);
  assert.deepEqual(whole.plan.check, { sha256: image.digest, bytes: 4_096 - 32, against: "digest" });
  assert.deepEqual(whole.plan.expected, { sha256: image.fileSha256, bytes: 4_096 });

  const part = planDownload({ region: "app_current", what: { kind: "range", span: { first: 16, last: 31 } } }, facts);
  assert.ok(part.ok);
  assert.equal(part.plan.check, undefined);
  assert.equal(part.plan.size, 16);

  const boot = planDownload({ region: "bootloader", what: { kind: "whole" } }, facts);
  assert.ok(boot.ok);
  assert.equal(boot.plan.filename, `${DEVICE}-bootloader.bin`);
  assert.equal(boot.plan.check?.against, "digest");
});

test("an empty, invalid or checking slot plans what it can", () => {
  const facts = (flash = internalFlash()) => ({ flash, regions: regions(), device: DEVICE });
  const empty = planDownload({ region: "app_alternate", what: { kind: "image" } }, facts());
  assert.deepEqual(empty, { ok: false, reason: "The slot holds no image." });
  const wholeEmpty = planDownload({ region: "app_alternate", what: { kind: "whole" } }, facts());
  assert.ok(wholeEmpty.ok);
  assert.equal(wholeEmpty.plan.filename, `${DEVICE}-app_alternate-ota_1-0x0-0x2fffff.bin`);

  const checking = internalFlash();
  const app = checking.apps[0];
  assert.ok(app);
  app.image = { ...app.image, status: "checking", digest: null, file_sha256: null, checked_uptime_s: null };
  const plan = planDownload({ region: "app_current", what: { kind: "image" } }, facts(checking));
  assert.ok(plan.ok);
  assert.equal(plan.plan.check, undefined);
  assert.equal(plan.plan.filename, `${DEVICE}-app_current-ota_0-0x0-0xfff.bin`);

  const invalid = internalFlash();
  const bad = invalid.apps[0];
  assert.ok(bad);
  bad.image = { ...bad.image, status: "invalid", error: "digest_mismatch", digest: null, file_sha256: null };
  const broken = planDownload({ region: "app_current", what: { kind: "image" } }, facts(invalid));
  assert.ok(broken.ok);
  assert.equal(broken.plan.check, undefined);
});

test("locked regions and ranges outside the region aren't planned", () => {
  const facts = { flash: internalFlash(), regions: regions(), device: DEVICE };
  const sensitive = planDownload({ region: "config_current", what: { kind: "whole" } }, facts);
  assert.equal(sensitive.ok, false);
  const outside = planDownload({ region: "otadata", what: { kind: "range", span: { first: 0, last: 0x2000 } } }, facts);
  assert.deepEqual(outside, { ok: false, reason: "The range is outside the region." });
  const untrusted = { flash: internalFlash("untrusted"), regions: regions({ tier: "untrusted" }), device: DEVICE };
  assert.deepEqual(planDownload({ region: "app_alternate", what: { kind: "whole" } }, untrusted), {
    ok: false,
    reason: "Not readable in the untrusted tier.",
  });
});

test("the full tier is normal and needs no banner", () => {
  assert.deepEqual(healthNotices(internalFlash("full")), []);
  assert.deepEqual(flashSummary(internalFlash("full")), ["Full tier", "8 MiB", "configuration saved", "updates ready"]);
});

test("the data tier says updates are unavailable, and why", () => {
  const [notice] = healthNotices(internalFlash("data"));
  assert.equal(notice?.level, "caution");
  assert.equal(notice?.title, "Updates are unavailable");
  assert.match(notice?.lines[0] ?? "", /isn't one this firmware knows/);
});

test("the untrusted tier is serious and names the likely cause", () => {
  const [notice] = healthNotices(internalFlash("untrusted"));
  assert.equal(notice?.level, "serious");
  assert.equal(notice?.title, "Running without saved configuration");
  assert.ok(notice?.lines.some((line) => /cargo run/.test(line)));
  assert.deepEqual(flashSummary(internalFlash("untrusted")).slice(2), ["configuration not saved", "no updates"]);
});

test("an invalid slot is serious, a boot needing confirmation is a caution, and running checks are said", () => {
  const flash = internalFlash("full", appImage(), {
    checks: { status: "running", finished_uptime_s: null },
  });
  const app = flash.apps[0];
  assert.ok(app && flash.boot);
  app.image = { ...app.image, status: "invalid", error: "checksum" };
  flash.boot.needs_confirmation = true;
  const notices = healthNotices(flash);
  assert.deepEqual(
    notices.map((notice) => [notice.level, notice.key]),
    [
      ["serious", "invalid-app_current"],
      ["caution", "confirm"],
      ["standby", "checks"],
    ],
  );
  assert.match(notices[0]?.title ?? "", /app_current \(ota_0\)/);
});

test("a differing table is compared entry by entry, with the extra and missing ones", () => {
  const rows = compareEntries(layoutEntries(regions()), boardTable(true).table?.entries ?? []);
  assert.equal(rows.length, 8);
  assert.deepEqual(rows[3]?.differs, ["offset", "size"]);
  assert.deepEqual(rows[0]?.differs, []);
  assert.equal(rows[7]?.found, undefined);
  const extra = compareEntries(layoutEntries(regions()).slice(0, 2), boardTable().table?.entries ?? []);
  assert.equal(extra[2]?.expected, undefined);
});

test("offsets and ranges are read in decimal or hex", () => {
  assert.equal(parseOffset("4096"), 4_096);
  assert.equal(parseOffset("0x1000"), 4_096);
  assert.equal(parseOffset("0X1_000"), 4_096);
  assert.equal(parseOffset("12abc"), undefined);
  assert.deepEqual(parseSpan("0x10", "", 0x2000), { ok: true, span: { first: 16, last: 0x1fff } });
  assert.equal(parseSpan("0x2000", "", 0x2000).ok, false);
  assert.equal(parseSpan("10", "5", 0x2000).ok, false);
  assert.equal(parseSpan("x", "5", 0x2000).ok, false);
});

test("rows say what each region is and what can be downloaded", () => {
  const flash = internalFlash();
  const areas = withGaps(regions(), 0x80_0000);
  const rows = areas.map((area) => regionRow(area, flash));
  const current = rows.find((row) => row.key === "app_current");
  assert.equal(current?.offset, "0x010000");
  assert.equal(current?.size, "3 MiB");
  assert.equal(current?.current, true);
  assert.deepEqual(current?.choices.map((choice) => choice.key), ["image", "whole", "range"]);
  assert.match(rows.find((row) => row.key === "diag")?.lock ?? "", /never leaves/);
  assert.equal(rows.find((row) => row.key === "gap-49152")?.name, "(unallocated)");
  const empty = regions()[5];
  assert.ok(empty);
  assert.deepEqual(downloadChoices(empty, flash).map((choice) => choice.key), ["whole", "range"]);
});

test("firmware cards say when the bootloader fell back from a slot", () => {
  const flash = internalFlash();
  assert.ok(flash.boot);
  flash.boot.selected = "ota_1";
  const list = regions();
  const alternate = list[5];
  const current = list[4];
  assert.ok(alternate && current);
  alternate.roles = ["selected"];
  current.roles = ["running"];
  const app = flash.apps[1];
  assert.ok(app);
  const view = firmwareView(app, flash, list);
  assert.equal(view.title, "Alternate (ota_1)");
  assert.match(view.fellBack ?? "", /fell back/);
  const running = firmwareView(flash.apps[0] ?? app, flash, list);
  assert.deepEqual(running.badges, [{ label: "Running", accent: true }]);
  assert.equal(running.name, "eveningstar 0.1.0");
  assert.equal(running.fill?.text, "4 KiB of 3 MiB");
  const chain = bootChain(flash);
  assert.match(chain.find((line) => line.label === "Booted")?.text ?? "", /otadata selects ota_1/);
  // An update written and not booted yet is selected by a `new` entry: it is waiting, not a fallback.
  flash.otadata = [
    { sector: 0, validity: "valid", seq: 3, state: "valid", selects: "ota_0" },
    { sector: 1, validity: "valid", seq: 4, state: "new", selects: "ota_1" },
  ];
  assert.equal(firmwareView(app, flash, list).fellBack, undefined);
});
