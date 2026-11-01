import assert from "node:assert/strict";
import { test } from "node:test";
import { signal } from "@preact/signals";
import type { ApiClient, Result } from "../api/client.ts";
import type { FailureContext } from "../api/failure.ts";
import { FakeClock, settle } from "../testing/fakes.ts";
import { boardTable, internalFlash, regions } from "../testing/storage.ts";
import { CHECKS_POLL_MS, createStorage } from "./service.ts";
import type { InternalFlash } from "./types.ts";

const context: FailureContext = { method: "GET", path: "/x", attempts: 1, at: 0 };

function setup(flashes: InternalFlash[]) {
  const clock = new FakeClock();
  const active = signal(true);
  const restarts = signal(0);
  const calls: string[] = [];
  let last = flashes[0] ?? internalFlash();
  const api = {
    request: (operation: string): Promise<Result<unknown>> => {
      calls.push(operation);
      switch (operation) {
        case "GetStorage":
          return Promise.resolve({
            ok: true,
            value: {
              devices: [
                { id: "internal", kind: "nor_flash", status: "available", capacity: 0x80_0000 },
                { id: "external", kind: "nand_flash", status: "not_supported", capacity: null },
              ],
            },
            context,
          });
        case "GetInternalFlash":
          last = flashes.shift() ?? last;
          return Promise.resolve({ ok: true, value: last, context });
        case "GetRegions":
          return Promise.resolve({ ok: true, value: { regions: regions() }, context });
        case "GetPartitionTable":
          return Promise.resolve({ ok: true, value: boardTable(), context });
        default:
          return Promise.resolve({ ok: false, failure: { kind: "network" }, context });
      }
    },
  } as unknown as ApiClient;
  const storage = createStorage({
    api,
    timers: clock,
    now: () => clock.now,
    active,
    restarts,
    deviceName: () => Promise.resolve("eveningstar-0kd7-9c6"),
  });
  return { clock, active, restarts, calls, storage };
}

test("the page loads the devices, the flash, its regions and the board's table, in order", async () => {
  const { calls, storage } = setup([internalFlash()]);
  for (const load of storage.loaders("storage")) assert.equal((await load()).ok, true);
  assert.deepEqual(calls, ["GetStorage", "GetInternalFlash", "GetRegions", "GetPartitionTable"]);
  assert.equal(storage.flash.value?.tier, "full");
  assert.equal(storage.regions.value?.length, 10);
  assert.equal(storage.table.value?.search, "compiled");
  assert.equal(storage.devices.value?.devices[1]?.status, "not_supported");
});

test("while the checks run, the flash is read again every few seconds until they are done", async () => {
  const running = internalFlash("full", undefined, { checks: { status: "running", finished_uptime_s: null } });
  const { clock, calls, storage, active } = setup([running, running, internalFlash()]);
  for (const load of storage.loaders("storage")) await load();
  const stop = storage.watch();
  calls.length = 0;
  await clock.advance(CHECKS_POLL_MS);
  assert.deepEqual(calls, ["GetInternalFlash", "GetRegions"]);
  assert.equal(storage.flash.value?.checks.status, "running");
  await clock.advance(CHECKS_POLL_MS);
  assert.equal(storage.flash.value?.checks.status, "done");
  await clock.advance(CHECKS_POLL_MS * 5);
  assert.equal(calls.length, 4);
  active.value = false;
  stop();
  assert.equal(clock.waiting, 0);
});

test("nothing is polled once the checks are done, and leaving the page stops the polling", async () => {
  const running = internalFlash("full", undefined, { checks: { status: "running", finished_uptime_s: null } });
  const { clock, calls, storage } = setup([running]);
  for (const load of storage.loaders("storage")) await load();
  const stop = storage.watch();
  stop();
  calls.length = 0;
  await clock.advance(CHECKS_POLL_MS * 3);
  assert.deepEqual(calls, []);
});

test("a restart of the device reads everything again", async () => {
  const { calls, storage, restarts } = setup([internalFlash()]);
  for (const load of storage.loaders("storage")) await load();
  const stop = storage.watch();
  calls.length = 0;
  restarts.value = 1;
  await settle();
  assert.deepEqual(calls, ["GetStorage", "GetInternalFlash", "GetRegions", "GetPartitionTable"]);
  stop();
});

test("refresh reads everything again and says it is refreshing", async () => {
  const { calls, storage } = setup([internalFlash()]);
  const done = storage.refresh("action");
  assert.equal(storage.refreshing.value, true);
  await done;
  assert.equal(storage.refreshing.value, false);
  assert.equal(calls.length, 4);
});

test("a download is planned against the flash as it is read when the download starts", async () => {
  const { calls, storage } = setup([internalFlash("data"), internalFlash("full")]);
  for (const load of storage.loaders("storage")) await load();
  calls.length = 0;
  const plan = await storage.prepare({ region: "app_current", what: { kind: "image" } });
  assert.deepEqual(calls, ["GetInternalFlash", "GetRegions"]);
  assert.ok(plan.ok);
  assert.equal(storage.flash.value?.tier, "full");
  assert.match(plan.plan.filename, /^eveningstar-0kd7-9c6-app_current-ota_0-image-/);
});
