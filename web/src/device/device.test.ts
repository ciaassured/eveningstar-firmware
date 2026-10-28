import assert from "node:assert/strict";
import { test } from "node:test";
import { signal } from "@preact/signals";
import type { ApiClient, Result } from "../api/client.ts";
import type { FailureContext } from "../api/failure.ts";
import { FakeClock } from "../testing/fakes.ts";
import { POLL_INTERVAL_MS, createDevice } from "./device.ts";
import type { RebootStatus, System } from "./types.ts";

const context: FailureContext = { method: "GET", path: "/x", attempts: 1, at: 0 };

function setup(uptimes: number[]) {
  const clock = new FakeClock();
  const active = signal(true);
  const calls: string[] = [];
  const idle: RebootStatus = { pending: null, holds: [] };
  const options: { operation: string; options: unknown }[] = [];
  const api = {
    request: (operation: string, given?: unknown): Promise<Result<unknown>> => {
      calls.push(operation);
      options.push({ operation, options: given });
      if (operation === "GetSystem") {
        const uptime_s = uptimes.shift() ?? 0;
        const value: System = { id: "4WSG-0KD7-9C6", name: "eveningstar-0kd7-9c6", uptime_s };
        return Promise.resolve({ ok: true, value, context });
      }
      return Promise.resolve({ ok: true, value: idle, context });
    },
  } as ApiClient;
  const device = createDevice({ api, timers: clock, now: () => clock.now, active });
  return { clock, active, calls, options, device };
}

test("the device polls the system and the reboot status", async () => {
  const { clock, calls, device } = setup([100, 105]);
  device.start();
  await clock.advance(POLL_INTERVAL_MS);
  assert.equal(device.system.value.value?.uptime_s, 100);
  assert.deepEqual(device.rebootStatus.value.value, { pending: null, holds: [] });
  await clock.advance(POLL_INTERVAL_MS);
  assert.equal(device.system.value.value?.uptime_s, 105);
  assert.equal(calls.filter((call) => call === "GetSystem").length, 2);
  assert.equal(device.restarts.value, 0);
  device.stop();
});

test("an uptime that goes down counts as a restart and refreshes the reboot status", async () => {
  const { clock, calls, device } = setup([5_000, 12]);
  device.start();
  await clock.advance(POLL_INTERVAL_MS);
  const before = calls.filter((call) => call === "GetRebootStatus").length;
  await clock.advance(POLL_INTERVAL_MS);
  assert.equal(device.restarts.value, 1);
  assert.ok(calls.filter((call) => call === "GetRebootStatus").length > before);
  device.stop();
});

test("reset forgets what was fetched, and the next fetch isn't taken for a restart", async () => {
  const { clock, device } = setup([5_000, 12]);
  device.start();
  await clock.advance(POLL_INTERVAL_MS);
  device.reset();
  const uptime = () => device.system.value.value?.uptime_s;
  assert.equal(uptime(), undefined);
  assert.equal(device.rebootStatus.value.value, undefined);
  await clock.advance(POLL_INTERVAL_MS);
  assert.equal(uptime(), 12);
  assert.equal(device.restarts.value, 0);
  device.stop();
});

test("the reboot flow asks the device without the transport's retries, so a device that is down shows as down", async () => {
  const { clock, options, device } = setup([5_000, 12]);
  const flow = device.reboot.start("graceful", 5_000);
  await clock.advance(POLL_INTERVAL_MS);
  await flow;
  const polls = options.filter((call) => call.operation !== "Reboot");
  assert.ok(polls.length >= 2);
  for (const poll of polls) assert.deepEqual(poll.options, { priority: "view", retry: false });
  const reboot = options.find((call) => call.operation === "Reboot");
  assert.deepEqual(reboot?.options, { priority: "action", body: { mode: "graceful" } });
});
