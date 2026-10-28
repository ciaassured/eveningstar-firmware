import assert from "node:assert/strict";
import { test } from "node:test";
import { signal } from "@preact/signals";
import type { Result } from "../api/client.ts";
import type { ApiFailure, FailureContext } from "../api/failure.ts";
import { FakeClock, settle } from "../testing/fakes.ts";
import { IDLE_AFTER_MS, createActivity } from "./activity.ts";
import { OFFLINE_AFTER_MS, createConnection } from "./connection.ts";
import { STALE_AFTER_INTERVALS, freshness } from "./freshness.ts";
import { createPoller } from "./poller.ts";
import { createTicker } from "./ticker.ts";

const context: FailureContext = { method: "GET", path: "/x", attempts: 1, at: 0 };
const ok = <T>(value: T): Result<T> => ({ ok: true, value, context });
const failed = <T>(failure: ApiFailure): Result<T> => ({ ok: false, failure, context });

test("connection is online until a request fails, degraded after one, offline after three", () => {
  const clock = new FakeClock();
  const connection = createConnection(() => clock.now);
  assert.equal(connection.state.value, "online");
  connection.report(false);
  assert.equal(connection.state.value, "degraded");
  connection.report(false);
  assert.equal(connection.state.value, "degraded");
  connection.report(false);
  assert.equal(connection.state.value, "offline");
  connection.report(true);
  assert.equal(connection.state.value, "online");
});

test("connection is offline once requests have failed for over 15 s", () => {
  const clock = new FakeClock();
  const connection = createConnection(() => clock.now);
  connection.report(false);
  clock.now = OFFLINE_AFTER_MS - 1;
  connection.report(false);
  assert.equal(connection.state.value, "degraded");
  clock.now = OFFLINE_AFTER_MS;
  connection.report(false);
  assert.equal(connection.state.value, "offline");
});

test("a success resets the count of failures", () => {
  const clock = new FakeClock();
  const connection = createConnection(() => clock.now);
  connection.report(false);
  connection.report(false);
  connection.report(true);
  connection.report(false);
  connection.report(false);
  assert.equal(connection.state.value, "degraded");
});

test("freshness is unknown before the first fetch, and stale after three intervals", () => {
  assert.equal(freshness(undefined, 1_000, 5_000), undefined);
  assert.deepEqual(freshness(0, 15_000, 5_000), { ageMs: 15_000, stale: false });
  assert.deepEqual(freshness(0, 15_001, 5_000), { ageMs: 15_001, stale: true });
  assert.equal(STALE_AFTER_INTERVALS, 3);
  assert.deepEqual(freshness(10, 5, 5_000), { ageMs: 0, stale: false });
});

function activityEnvironment(clock: FakeClock) {
  let visible = true;
  const visibility = new Set<() => void>();
  const input = new Set<() => void>();
  return {
    environment: {
      visible: () => visible,
      onVisibilityChange: (listener: () => void) => {
        visibility.add(listener);
        return () => visibility.delete(listener);
      },
      onInput: (listener: () => void) => {
        input.add(listener);
        return () => input.delete(listener);
      },
      timers: clock,
    },
    hide() {
      visible = false;
      visibility.forEach((listener) => listener());
    },
    show() {
      visible = true;
      visibility.forEach((listener) => listener());
    },
    type() {
      input.forEach((listener) => listener());
    },
    listeners: () => visibility.size + input.size,
  };
}

test("activity follows visibility", () => {
  const clock = new FakeClock();
  const page = activityEnvironment(clock);
  const activity = createActivity(page.environment);
  assert.equal(activity.active.value, true);
  page.hide();
  assert.equal(activity.active.value, false);
  page.show();
  assert.equal(activity.active.value, true);
});

test("activity goes idle after 30 minutes without input, and only resume wakes it", async () => {
  const clock = new FakeClock();
  const page = activityEnvironment(clock);
  const activity = createActivity(page.environment);

  await clock.advance(IDLE_AFTER_MS - 1);
  page.type();
  await clock.advance(IDLE_AFTER_MS - 1);
  assert.equal(activity.idle.value, false);
  await clock.advance(1);
  assert.equal(activity.idle.value, true);
  assert.equal(activity.active.value, false);

  page.type();
  assert.equal(activity.idle.value, true);
  activity.resume();
  assert.equal(activity.active.value, true);
  await clock.advance(IDLE_AFTER_MS);
  assert.equal(activity.idle.value, true);

  activity.dispose();
  assert.equal(page.listeners(), 0);
});

function pollerSetup(results: Result<number>[]) {
  const clock = new FakeClock();
  const active = signal(true);
  const priorities: string[] = [];
  const queue = [...results];
  const poller = createPoller<number>({
    fetch: (priority) => {
      priorities.push(priority);
      return Promise.resolve(queue.shift() ?? failed({ kind: "network" }));
    },
    intervalMs: 5_000,
    maxIntervalMs: 30_000,
    timers: clock,
    now: () => clock.now,
    active,
  });
  return { clock, active, poller, priorities };
}

test("the poller waits an interval before its first poll, then polls every interval, and records when", async () => {
  const { clock, poller, priorities } = pollerSetup([ok(1), ok(2), ok(3)]);
  poller.start();
  await clock.advance(4_999);
  assert.equal(priorities.length, 0);
  await clock.advance(1);
  assert.equal(poller.value.value, 1);
  assert.equal(poller.updatedAt.value, 5_000);
  await clock.advance(5_000);
  assert.equal(poller.value.value, 2);
  assert.equal(poller.updatedAt.value, 10_000);
  assert.deepEqual(priorities, ["poll", "poll"]);
  poller.stop();
  await clock.advance(60_000);
  assert.equal(priorities.length, 2);
});

test("the poller backs off to 30 s while fetches fail, and a success restores the interval", async () => {
  const net = failed<number>({ kind: "network" });
  const { clock, poller, priorities } = pollerSetup([ok(1), net, net, net, net, ok(2), ok(3)]);
  poller.start();
  await clock.advance(5_000);
  assert.equal(priorities.length, 1);
  // Failures wait 10, 20, 30, 30 s.
  await clock.advance(5_000);
  assert.equal(priorities.length, 2);
  assert.deepEqual(poller.error.value, { kind: "network" });
  assert.equal(poller.value.value, 1);
  await clock.advance(10_000);
  assert.equal(priorities.length, 3);
  await clock.advance(20_000);
  assert.equal(priorities.length, 4);
  await clock.advance(30_000);
  assert.equal(priorities.length, 5);
  await clock.advance(30_000);
  assert.equal(priorities.length, 6);
  assert.equal(poller.value.value, 2);
  assert.equal(poller.error.value, undefined);
  await clock.advance(5_000);
  assert.equal(priorities.length, 7);
  poller.stop();
});

test("the poller stops while the page isn't in use and refreshes at once when it is again", async () => {
  const { clock, active, poller, priorities } = pollerSetup([ok(1), ok(2), ok(3)]);
  poller.start();
  await clock.advance(5_000);
  assert.equal(priorities.length, 1);
  active.value = false;
  await clock.advance(60_000);
  assert.equal(priorities.length, 1);
  active.value = true;
  await settle();
  assert.equal(priorities.length, 2);
  assert.equal(poller.value.value, 2);
  poller.stop();
});

test("a poller started while the page is hidden polls at once when it is shown", async () => {
  const { clock, active, poller, priorities } = pollerSetup([ok(1), ok(2)]);
  active.value = false;
  poller.start();
  await clock.advance(30_000);
  assert.equal(priorities.length, 0);
  active.value = true;
  await settle();
  assert.equal(priorities.length, 1);
  poller.stop();
});

test("the ticker moves the time on every second while the page is in use", async () => {
  const clock = new FakeClock();
  const active = signal(true);
  const time = createTicker(clock, () => clock.now, active);
  await clock.advance(3_000);
  assert.equal(time.value, 3_000);
  active.value = false;
  await clock.advance(5_000);
  assert.equal(time.value, 3_000);
  active.value = true;
  assert.equal(time.value, 8_000);
});

test("load fetches with the given priority and updates the signals", async () => {
  const { poller, priorities } = pollerSetup([ok(7), failed({ kind: "timeout", afterMs: 8_000 })]);
  const first = await poller.load("view");
  assert.equal(first.ok, true);
  assert.equal(poller.value.value, 7);
  const second = await poller.load("view");
  assert.equal(second.ok, false);
  assert.deepEqual(poller.error.value, { kind: "timeout", afterMs: 8_000 });
  assert.equal(poller.value.value, 7);
  assert.deepEqual(priorities, ["view", "view"]);
});

test("an aborted fetch is not an error", async () => {
  const { poller } = pollerSetup([failed({ kind: "aborted" })]);
  await poller.load("view");
  assert.equal(poller.error.value, undefined);
});
