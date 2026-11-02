import assert from "node:assert/strict";
import { test } from "node:test";
import { formatClock } from "../lib/format.ts";
import { planDownload } from "../storage/plan.ts";
import type { DownloadPlan, DownloadTarget } from "../storage/plan.ts";
import { TABLE_MD5, appImage, storageInput } from "../testing/storage.ts";
import type { DownloadEntry, EntryState } from "./service.ts";
import { buttonView, finishedIds, queuedAge, statusOf, trayCount, trayItem, trayItems } from "./view.ts";
import type { ViewFacts } from "./view.ts";

const DEVICE = "eveningstar-0kd7-9c6";
const NOW = 1_800_000_000_000;
const input = storageInput();
const image: DownloadTarget = { region: "app_current", what: { kind: "image" } };
const table: DownloadTarget = { region: "partition_table", what: { kind: "whole" } };
const otadata: DownloadTarget = { region: "otadata", what: { kind: "whole" } };

function plan(target: DownloadTarget): DownloadPlan {
  const result = planDownload(target, { flash: input.flash, regions: input.regions, device: DEVICE, table: input.table });
  assert.ok(result.ok);
  return result.plan;
}

const facts: ViewFacts = { now: NOW, regions: input.regions, preview: (target) => plan(target) };

let ids = 0;
function entry(state: EntryState, target = image, extra: Partial<DownloadEntry> = {}): DownloadEntry {
  ids += 1;
  return { id: ids, target, queuedAt: NOW - 5_000, state, plan: plan(target), ...extra };
}

const SIZE = "4 KiB";
const FILE = `${DEVICE}-app_current-ota_0-image-${appImage().fileSha256.slice(0, 8)}.bin`;

test("each service state maps onto one of the five statuses", () => {
  const cases: [EntryState, string][] = [
    [{ kind: "queued" }, "pending"],
    [{ kind: "waiting", since: 0, until: 1 }, "pending"],
    [{ kind: "downloading", received: 0, total: undefined, retrying: false }, "downloading"],
    [{ kind: "downloading", received: 1, total: 2, retrying: true }, "downloading"],
    [{ kind: "verifying" }, "downloading"],
    [{ kind: "done", verified: "checked" }, "completed"],
    [{ kind: "done", verified: "unchecked" }, "completed"],
    [{ kind: "done", verified: "mismatch" }, "failed"],
    [{ kind: "failed", reason: "", resumable: true, received: 0 }, "failed"],
    [{ kind: "cancelled" }, "cancelled"],
  ];
  for (const [state, status] of cases) assert.equal(statusOf(state), status, state.kind);
});

test("the word, the amount, the progress and the actions of each state", () => {
  const row = (state: EntryState) => {
    const item = trayItem(entry(state), facts);
    return [item.word, item.amount, item.progress, item.actions.join(" ")];
  };
  assert.deepEqual(row({ kind: "queued" }), ["Pending", SIZE, SIZE, "cancel"]);
  assert.deepEqual(row({ kind: "waiting", since: NOW - 1_000, until: NOW + 4_000 }), ["Pending", "Waiting", SIZE, "cancel"]);
  assert.deepEqual(row({ kind: "downloading", received: 0, total: undefined, retrying: false }), ["Downloading", "Preparing", SIZE, "cancel"]);
  assert.deepEqual(row({ kind: "downloading", received: 1_536, total: 4_096, retrying: false }), ["Downloading", "37%", "1.5 KiB of 4 KiB", "cancel"]);
  assert.deepEqual(row({ kind: "verifying" }), ["Downloading", "Checking", "4 KiB of 4 KiB", "cancel"]);
  assert.deepEqual(row({ kind: "done", verified: "checked" }), ["Completed", SIZE, "4 KiB saved", "remove"]);
  assert.deepEqual(row({ kind: "done", verified: "mismatch" }), ["Failed", "Unverified", "4 KiB of 4 KiB", "retry remove"]);
  assert.deepEqual(row({ kind: "failed", reason: "x", resumable: true, received: 1_024 }), ["Failed", "at 25%", "1 KiB of 4 KiB", "retry remove"]);
  assert.deepEqual(
    row({ kind: "failed", reason: "x", resumable: false, received: 4_096, mismatch: { expected: "a", actual: "b" } }),
    ["Failed", "at 100%", "4 KiB of 4 KiB", "retry save-anyway remove"],
  );
  assert.deepEqual(row({ kind: "cancelled" }), ["Cancelled", SIZE, SIZE, "retry remove"]);
});

test("a running download has its share, and a failed one its reason", () => {
  const running = trayItem(entry({ kind: "downloading", received: 1_024, total: 4_096, retrying: false }), facts);
  assert.equal(running.share, 0.25);
  assert.equal(trayItem(entry({ kind: "verifying" }), facts).share, 1);
  assert.equal(trayItem(entry({ kind: "queued" }), facts).share, undefined);
  assert.equal(trayItem(entry({ kind: "failed", reason: "The device refused.", resumable: false, received: 0 }), facts).reason, "The device refused.");
});

test("the notes: waiting, resuming, saved unverified, why it started again, and the download's own", () => {
  const notes = (state: EntryState, target = image, extra: Partial<DownloadEntry> = {}) => trayItem(entry(state, target, extra), facts).notes;
  assert.deepEqual(notes({ kind: "waiting", since: NOW - 1_000, until: NOW + 65_000 }), [
    "Another client is downloading from the device. Asking again in 1:05.",
  ]);
  assert.deepEqual(notes({ kind: "downloading", received: 2_048, total: 4_096, retrying: true }), [
    "The connection broke; the download resumes from 2 KiB.",
  ]);
  assert.deepEqual(notes({ kind: "done", verified: "mismatch" }), [
    "Saved as unverified: the SHA-256 of the received bytes does not match the device's.",
  ]);
  assert.deepEqual(notes({ kind: "queued" }, otadata, { note: "The region changed since the download started, so it started again." }), [
    "The region changed since the download started, so it started again.",
    "No hash: the device does not hash whole regions.",
  ]);
  assert.deepEqual(notes({ kind: "done", verified: "checked" }), []);
});

test("the file name comes from the plan, or for a download not started from the preview, and says when it is unverified", () => {
  assert.equal(trayItem(entry({ kind: "done", verified: "checked" }), facts).filename, FILE);
  const queued: DownloadEntry = { id: 99, target: image, queuedAt: NOW, state: { kind: "queued" } };
  assert.equal(trayItem(queued, facts).filename, FILE);
  assert.equal(trayItem(queued, { ...facts, preview: () => undefined }).filename, "App image, ota_0");
  assert.equal(trayItem(entry({ kind: "done", verified: "mismatch" }), facts).filename, FILE.replace(/\.bin$/, "-UNVERIFIED.bin"));
});

test("the details: when it was queued, where it is from, the range, and the hash", () => {
  const item = trayItem(entry({ kind: "queued" }), facts);
  assert.equal(item.queued, `${formatClock(NOW - 5_000)}, just now`);
  assert.deepEqual(item.from, { code: "A0", colour: "--r-a0", title: "ota_0", subtitle: "Running app" });
  const slot = input.regions.find((info) => info.region === "app_current");
  assert.ok(slot);
  assert.equal(item.span?.start.decimal, new Intl.NumberFormat("en-US").format(slot.offset));
  assert.equal(item.span?.size.text, "4,096 bytes, 4 KiB");
  assert.deepEqual(item.hash, { label: "SHA-256 of the image file", text: "the value for the file you receive", value: appImage().fileSha256 });
  const tableItem = trayItem(entry({ kind: "queued" }, table), facts);
  assert.equal(tableItem.what, "Partition table, entries and MD5 entry");
  assert.deepEqual(tableItem.hash, { label: "Stored MD5", text: "over the entries; the MD5 entry is part of the file", value: TABLE_MD5 });
  assert.equal(trayItem(entry({ kind: "queued" }, otadata), facts).hash, undefined);
});

test("the age of a download: just now, seconds, minutes, then hours", () => {
  assert.equal(queuedAge(9_999), "just now");
  assert.equal(queuedAge(10_000), "10 s ago");
  assert.equal(queuedAge(59_999), "59 s ago");
  assert.equal(queuedAge(60_000), "1 min ago");
  assert.equal(queuedAge(3_599_999), "59 min ago");
  assert.equal(queuedAge(7_200_000), "2 h ago");
});

test("newest first; the count; Clear finished takes completed and cancelled ones and leaves failed ones", () => {
  const entries = [
    entry({ kind: "done", verified: "checked" }),
    entry({ kind: "done", verified: "mismatch" }),
    entry({ kind: "failed", reason: "x", resumable: true, received: 0 }),
    entry({ kind: "cancelled" }),
    entry({ kind: "downloading", received: 0, total: 4_096, retrying: false }),
    entry({ kind: "queued" }),
  ];
  const items = trayItems(entries, facts);
  assert.deepEqual(items.map((item) => item.id), entries.map((item) => item.id).reverse());
  assert.equal(trayCount(items), "2 active, 4 finished");
  assert.deepEqual(finishedIds(items).sort(), [entries[0]?.id, entries[3]?.id].sort());
});

test("the Downloads button's badge, ring and accessible name", () => {
  assert.deepEqual(buttonView([]), { active: 0, share: 0, label: "Downloads, none active" });
  const done = trayItems([entry({ kind: "done", verified: "checked" })], facts);
  assert.deepEqual(buttonView(done), { active: 0, share: 0, label: "Downloads, none active" });
  const waiting = trayItems([entry({ kind: "waiting", since: 0, until: NOW }), entry({ kind: "queued" })], facts);
  assert.deepEqual(buttonView(waiting), { active: 2, share: 0, label: "Downloads, 2 active" });
  const running = trayItems([entry({ kind: "downloading", received: 1_536, total: 4_096, retrying: false }), entry({ kind: "queued" })], facts);
  assert.deepEqual(buttonView(running), { active: 2, share: 0.375, label: `Downloads, 2 active, 37% of ${FILE}` });
});
