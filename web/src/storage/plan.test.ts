import assert from "node:assert/strict";
import { test } from "node:test";
import { appImage, internalFlash, regions } from "../testing/storage.ts";
import { downloadFilename, unverifiedFilename } from "./filenames.ts";
import { lockReason, planDownload } from "./plan.ts";

const DEVICE = "eveningstar-0kd7-9c6";

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

test("locked regions aren't planned", () => {
  const facts = { flash: internalFlash(), regions: regions(), device: DEVICE };
  const sensitive = planDownload({ region: "config_current", what: { kind: "whole" } }, facts);
  assert.equal(sensitive.ok, false);
  const untrusted = { flash: internalFlash("untrusted"), regions: regions({ tier: "untrusted" }), device: DEVICE };
  assert.deepEqual(planDownload({ region: "app_alternate", what: { kind: "whole" } }, untrusted), {
    ok: false,
    reason: "Not readable in the untrusted tier.",
  });
});
