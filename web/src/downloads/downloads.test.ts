import assert from "node:assert/strict";
import { test } from "node:test";
import { createApiClient } from "../api/client.ts";
import { STALL_MS, Transport } from "../api/transport.ts";
import { planDownload } from "../storage/plan.ts";
import type { DownloadTarget, PlanResult } from "../storage/plan.ts";
import type { InternalFlash } from "../storage/types.ts";
import { FakeClock, FakeFetch, jsonResponse, serveContent, settle } from "../testing/fakes.ts";
import type { ContentScript, Step } from "../testing/fakes.ts";
import { appImage, bytesOf, internalFlash, regions, sha256 } from "../testing/storage.ts";
import { BUSY_WAIT_S, MAX_RESUMES, RESUME_DELAYS_MS, createDownloads } from "./service.ts";
import type { DownloadsDeps } from "./service.ts";
import { entryViews, traySummary } from "./view.ts";

const DEVICE = "eveningstar-0kd7-9c6";
/** The longest resume delay, with the jitter the fixed random value gives. */
const LONGEST_WAIT = Math.max(...RESUME_DELAYS_MS) * 1.25;

function setup(options: { verifies?: boolean; flash?: InternalFlash } = {}) {
  const clock = new FakeClock();
  const fake = new FakeFetch();
  const transport = new Transport({
    fetch: fake.fetch,
    timers: clock,
    random: () => 0.5,
    now: () => clock.now,
    auth: { decorate: (init) => init, onUnauthorized: () => Promise.resolve("login-required") },
    onLoginRequired: () => {},
  });
  const facts = { flash: options.flash ?? internalFlash(), regions: regions(), device: DEVICE };
  const saved: { name: string; bytes: Uint8Array }[] = [];
  const guards: boolean[] = [];
  const planned: DownloadTarget[] = [];
  const deps: DownloadsDeps = {
    api: createApiClient(transport),
    prepare: (target): Promise<PlanResult> => {
      planned.push(target);
      return Promise.resolve(planDownload(target, facts));
    },
    timers: clock,
    now: () => clock.now,
    random: () => 0.5,
    save: (bytes, name) => saved.push({ name, bytes: bytes.slice() }),
    sha256: options.verifies === false ? undefined : (bytes) => Promise.resolve(sha256(bytes)),
    guardUnload: (on) => guards.push(on),
  };
  const downloads = createDownloads(deps);
  return { clock, fake, transport, facts, saved, guards, planned, downloads };
}

const image = (): DownloadTarget => ({ region: "app_current", what: { kind: "image" } });

/** The current slot's content: the image, then erased flash. */
function slotBytes(bytes = appImage().bytes): Uint8Array {
  const slot = new Uint8Array(0x30_0000).fill(0xff);
  slot.set(bytes);
  return slot;
}

function states(downloads: ReturnType<typeof setup>["downloads"]): string[] {
  return downloads.entries.value.map((entry) => entry.state.kind);
}

test("a download of the image is checked against its file_sha256 and saved under its name", async () => {
  const { fake, saved, downloads, clock } = setup();
  const slot = slotBytes();
  fake.reply(serveContent(slot, { etag: '"boot-0"' }));
  downloads.add(image());
  await clock.advance(1);
  assert.deepEqual(states(downloads), ["done"]);
  const entry = downloads.entries.value[0];
  assert.deepEqual(entry?.state, { kind: "done", verified: "checked" });
  assert.equal(saved.length, 1);
  assert.equal(saved[0]?.name, `${DEVICE}-app_current-ota_0-image-${appImage().fileSha256.slice(0, 8)}.bin`);
  assert.equal(sha256(saved[0]?.bytes ?? new Uint8Array()), appImage().fileSha256);
  const headers = fake.calls[0]?.init.headers as Record<string, string>;
  assert.equal(headers["Range"], "bytes=0-4095");
  assert.equal(headers["If-Range"], undefined);
});

test("a whole slot is checked by the image's stored digest", async () => {
  const { fake, saved, downloads, clock } = setup();
  fake.reply(serveContent(slotBytes(), { etag: '"boot-0"', chunk: 65_536 }));
  downloads.add({ region: "app_current", what: { kind: "whole" } });
  await clock.advance(1);
  assert.deepEqual(downloads.entries.value[0]?.state, { kind: "done", verified: "checked" });
  assert.equal(saved[0]?.bytes.byteLength, 0x30_0000);
  assert.match(saved[0]?.name ?? "", /-slot-of-/);
  assert.equal((fake.calls[0]?.init.headers as Record<string, string>)["Range"], undefined);
});

test("bytes that don't match aren't saved, unless the person saves them anyway", async () => {
  const { fake, saved, downloads, clock } = setup();
  const wrong = slotBytes(bytesOf(4_096, 99));
  fake.reply(serveContent(wrong, { etag: '"boot-0"' }));
  const id = downloads.add(image());
  await clock.advance(1);
  const state = downloads.entries.value[0]?.state;
  assert.equal(state?.kind, "failed");
  assert.equal(state?.kind === "failed" && state.mismatch?.actual, sha256(wrong.subarray(0, 4_096)));
  assert.equal(saved.length, 0);
  downloads.act(id, "save-anyway");
  assert.match(saved[0]?.name ?? "", /-UNVERIFIED\.bin$/);
  assert.deepEqual(downloads.entries.value[0]?.state, { kind: "done", verified: "mismatch" });
});

test("outside a secure context nothing is checked, and the list shows the SHA-256 to check by hand", async () => {
  const { fake, saved, downloads, clock } = setup({ verifies: false });
  fake.reply(serveContent(slotBytes(), { etag: '"boot-0"', chunk: 65_536 }));
  downloads.add({ region: "app_current", what: { kind: "whole" } });
  await clock.advance(1);
  assert.deepEqual(downloads.entries.value[0]?.state, { kind: "done", verified: "unchecked" });
  assert.equal(saved.length, 1);
  const [view] = entryViews(downloads.entries.value, clock.now, downloads.verifies);
  assert.equal(view?.expected?.sha256, appImage().fileSha256);
  assert.match(view?.expected?.command ?? "", /^head -c 4096 eveningstar-0kd7-9c6-app_current-ota_0-slot-of-\w{8}\.bin \| sha256sum$/);
});

test("the queue runs one download at a time, in order", async () => {
  const { fake, downloads, clock, transport } = setup();
  const slow = FakeFetch.deferred(new Response(slotBytes().slice(0, 4_096), { status: 206, headers: { "Content-Range": "bytes 0-4095/3145728", ETag: '"boot-0"' } }));
  const bootloader = new Uint8Array(0x8000).fill(0xff);
  bootloader.set(appImage(22_528, 3).bytes);
  fake.reply(slow.step, serveContent(new Uint8Array(0x2000), { etag: '"o-0"' }), serveContent(bootloader, { etag: '"b-0"' }));
  downloads.add(image());
  downloads.add({ region: "otadata", what: { kind: "whole" } });
  downloads.add({ region: "bootloader", what: { kind: "whole" } });
  await clock.advance(1);
  assert.deepEqual(states(downloads), ["downloading", "queued", "queued"]);
  assert.equal(fake.maxInFlight, 1);
  assert.equal(downloads.queued.value, 2);
  assert.equal(traySummary(downloads.entries.value, clock.now), "0% · 2 queued");
  slow.release();
  await clock.advance(1);
  assert.deepEqual(states(downloads), ["done", "done", "done"]);
  assert.deepEqual(
    fake.calls.map((call) => call.url.split("/").at(-2)),
    ["app_current", "otadata", "bootloader"],
  );
  assert.equal(transport.downloading, false);
});

test("queued entries can be reordered and cancelled", async () => {
  const { fake, downloads, clock } = setup();
  const slow = FakeFetch.deferred(jsonResponse(503, { error: "busy" }, { "Retry-After": "1" }));
  fake.reply(slow.step);
  downloads.add(image());
  const b = downloads.add({ region: "otadata", what: { kind: "whole" } });
  const c = downloads.add({ region: "bootloader", what: { kind: "whole" } });
  const d = downloads.add({ region: "partition_table", what: { kind: "whole" } });
  await clock.advance(1);
  downloads.act(c, "up");
  assert.deepEqual(downloads.entries.value.map((entry) => entry.id), [1, c, b, d]);
  downloads.act(c, "up");
  assert.deepEqual(downloads.entries.value.map((entry) => entry.id), [1, c, b, d], "the running entry keeps its place");
  downloads.act(b, "down");
  assert.deepEqual(downloads.entries.value.map((entry) => entry.id), [1, c, d, b]);
  downloads.act(d, "cancel");
  assert.equal(downloads.entries.value.find((entry) => entry.id === d)?.state.kind, "cancelled");
  const views = entryViews(downloads.entries.value, clock.now, true);
  assert.deepEqual(views.find((view) => view.id === c)?.actions, ["down", "cancel"]);
  assert.match(views.find((view) => view.id === c)?.status ?? "", /1st in line/);
  downloads.act(1, "cancel");
  slow.release();
  await settle();
  assert.equal(downloads.entries.value[0]?.state.kind, "cancelled");
});

test("a busy device makes the entry wait, as long as it takes, and that is never a failure", async () => {
  const { fake, downloads, clock } = setup();
  const busy: Step = () => jsonResponse(503, { error: "busy" }, { "Retry-After": "3" });
  const silent: Step = () => jsonResponse(503, { error: "busy" });
  fake.reply(busy, busy, busy, busy, busy, busy, silent, serveContent(slotBytes(), { etag: '"boot-0"' }));
  downloads.add(image());
  await clock.advance(1);
  const first = downloads.entries.value[0]?.state;
  assert.equal(first?.kind, "waiting");
  assert.equal(first?.kind === "waiting" && first.until - first.since, 3_000);
  assert.match(entryViews(downloads.entries.value, clock.now, true)[0]?.status ?? "", /^Waiting: another client is downloading/);
  for (let i = 0; i < 6; i += 1) {
    await clock.advance(3_000);
    assert.notEqual(downloads.entries.value[0]?.state.kind, "failed");
  }
  assert.equal(downloads.entries.value[0]?.state.kind, "waiting");
  await clock.advance(BUSY_WAIT_S * 1_000);
  assert.equal(downloads.entries.value[0]?.state.kind, "done");
  assert.equal(fake.calls.length, 8);
});

test("a stalled download resumes with Range and If-Range, keeping what arrived", async () => {
  const { fake, downloads, clock, saved } = setup();
  const slot = slotBytes();
  fake.reply(serveContent(slot, { etag: '"boot-0"', stallAfter: 1_500 }), serveContent(slot, { etag: '"boot-0"' }));
  downloads.add(image());
  await clock.advance(STALL_MS);
  const state = downloads.entries.value[0]?.state;
  assert.equal(state?.kind === "downloading" && state.retrying, true);
  await clock.advance(LONGEST_WAIT);
  const headers = fake.calls[1]?.init.headers as Record<string, string>;
  assert.equal(headers["Range"], "bytes=1500-4095");
  assert.equal(headers["If-Range"], '"boot-0"');
  assert.deepEqual(downloads.entries.value[0]?.state, { kind: "done", verified: "checked" });
  assert.equal(sha256(saved[0]?.bytes ?? new Uint8Array()), appImage().fileSha256);
});

test("a broken download is resumed a few times with backoff, then waits for the resume button", async () => {
  const { fake, downloads, clock, guards } = setup();
  const slot = slotBytes();
  const cut: ContentScript = { etag: '"boot-0"', cutAfter: 1_000 };
  fake.reply(serveContent(slot, cut));
  for (let i = 0; i < MAX_RESUMES; i += 1) fake.reply(FakeFetch.fail);
  const id = downloads.add(image());
  await clock.advance(1);
  for (let i = 0; i < MAX_RESUMES; i += 1) await clock.advance(LONGEST_WAIT);
  const state = downloads.entries.value[0]?.state;
  assert.deepEqual(state, { kind: "failed", reason: "The connection broke before the download finished.", resumable: true });
  assert.equal(fake.calls.length, 1 + MAX_RESUMES);
  assert.equal(guards.at(-1), true, "a resumable entry still keeps the tab from closing unasked");
  fake.reply(serveContent(slot, { etag: '"boot-0"' }));
  downloads.act(id, "resume");
  await clock.advance(1);
  assert.equal((fake.calls.at(-1)?.init.headers as Record<string, string>)["Range"], "bytes=1000-4095");
  assert.equal(downloads.entries.value[0]?.state.kind, "done");
  assert.equal(guards.at(-1), false);
});

test("a 200 to a resume means the region changed: the download is planned and started again, and says why", async () => {
  const { fake, downloads, clock, saved, facts, planned } = setup();
  const before = slotBytes();
  const next = appImage(4_096, 42);
  fake.reply(serveContent(before, { etag: '"boot-0"', cutAfter: 2_048 }), serveContent(slotBytes(next.bytes), { etag: '"boot-1"' }));
  fake.reply(serveContent(slotBytes(next.bytes), { etag: '"boot-1"' }));
  downloads.add(image());
  await clock.advance(1);
  facts.flash = internalFlash("full", next);
  await clock.advance(LONGEST_WAIT);
  const entry = downloads.entries.value[0];
  assert.deepEqual(entry?.state, { kind: "done", verified: "checked" });
  assert.match(entry?.note ?? "", /region changed/);
  assert.equal(planned.length, 2);
  assert.equal(saved[0]?.name, `${DEVICE}-app_current-ota_0-image-${next.fileSha256.slice(0, 8)}.bin`);
  assert.equal(sha256(saved[0]?.bytes ?? new Uint8Array()), next.fileSha256);
  assert.equal((fake.calls[2]?.init.headers as Record<string, string>)["If-Range"], undefined);
});

test("a device restart doesn't cancel the queue: the resume finds the new boot and starts again", async () => {
  const { fake, downloads, clock } = setup();
  const slot = slotBytes();
  fake.reply(
    serveContent(slot, { etag: '"boot-0"', cutAfter: 1_024 }),
    FakeFetch.fail,
    FakeFetch.fail,
    serveContent(slot, { etag: '"boot-1"' }),
    serveContent(slot, { etag: '"boot-1"' }),
  );
  downloads.add(image());
  downloads.add({ region: "otadata", what: { kind: "whole" } });
  fake.reply(serveContent(new Uint8Array(0x2000), { etag: '"boot-1"' }));
  await clock.advance(1);
  for (let i = 0; i < 4; i += 1) await clock.advance(LONGEST_WAIT);
  assert.deepEqual(states(downloads), ["done", "done"]);
  assert.match(downloads.entries.value[0]?.note ?? "", /started again/);
});

test("cancelling the running download aborts it and the next one starts", async () => {
  const { fake, downloads, clock } = setup();
  fake.reply(serveContent(slotBytes(), { etag: '"boot-0"', stallAfter: 1_024 }), serveContent(new Uint8Array(0x2000), { etag: '"o"' }));
  const first = downloads.add(image());
  downloads.add({ region: "otadata", what: { kind: "whole" } });
  await clock.advance(1);
  downloads.act(first, "cancel");
  await clock.advance(1);
  assert.deepEqual(states(downloads), ["cancelled", "done"]);
});

test("a refused download fails without a resume", async () => {
  const { fake, downloads, clock } = setup();
  fake.reply(() => jsonResponse(409, { error: "unavailable" }));
  downloads.add({ region: "otadata", what: { kind: "whole" } });
  await clock.advance(1);
  assert.deepEqual(downloads.entries.value[0]?.state, {
    kind: "failed",
    reason: "The firmware can't read the region this boot.",
    resumable: false,
  });
});

test("a download that can't be planned fails and says why", async () => {
  const { fake, downloads, clock } = setup();
  downloads.add({ region: "app_alternate", what: { kind: "image" } });
  await clock.advance(1);
  assert.deepEqual(downloads.entries.value[0]?.state, { kind: "failed", reason: "The slot holds no image.", resumable: false });
  assert.equal(fake.calls.length, 0);
});

test("the tab asks before closing only while the queue holds something, and dismissing clears it", async () => {
  const { fake, downloads, clock, guards } = setup();
  assert.deepEqual(guards, [false]);
  fake.reply(serveContent(slotBytes(), { etag: '"boot-0"' }));
  const id = downloads.add(image());
  assert.equal(downloads.pending.value, true);
  assert.equal(guards.at(-1), true);
  await clock.advance(1);
  assert.equal(downloads.pending.value, false);
  assert.equal(guards.at(-1), false);
  downloads.act(id, "dismiss");
  assert.deepEqual(downloads.entries.value, []);
});

test("downloads carry on when the page that started them is left", async () => {
  const { fake, downloads, clock, transport } = setup();
  const slow = FakeFetch.deferred(new Response(slotBytes().slice(0, 4_096), { status: 206, headers: { "Content-Range": "bytes 0-4095/3145728", ETag: '"boot-0"' } }));
  fake.reply(slow.step);
  downloads.add(image());
  await clock.advance(1);
  transport.abortGroup("storage");
  slow.release();
  await clock.advance(1);
  assert.equal(downloads.entries.value[0]?.state.kind, "done");
});
