import assert from "node:assert/strict";
import { test } from "node:test";
import type { Result } from "../api/client.ts";
import type { ApiFailure, FailureContext } from "../api/failure.ts";
import { FakeClock, settle } from "../testing/fakes.ts";
import { POLL_INTERVAL_MS, RESTART_TIMEOUT_MS, createRebootFlow } from "./reboot.ts";
import type { RebootProgress } from "./reboot.ts";
import type { RebootMode, RebootStatus, System } from "./types.ts";

const context: FailureContext = { method: "POST", path: "/x", attempts: 1, at: 0 };
const ok = <T>(value: T): Result<T> => ({ ok: true, value, context });
const fail = <T>(failure: ApiFailure): Result<T> => ({ ok: false, failure, context });
const system = (uptime_s: number): System => ({ id: "4WSG-0KD7-9C6", name: "eveningstar-0kd7-9c6", uptime_s });
const idle: RebootStatus = { pending: null, holds: [] };
const pending: RebootStatus = {
  pending: { reason: "api", remaining_s: 20 },
  holds: [{ holder: "http", count: 1 }],
};

/** A device that answers from scripts; the last answer of a script repeats. */
function setup(script: {
  reboot: Result<undefined>;
  status: Result<RebootStatus>[];
  system: Result<System>[];
}) {
  const clock = new FakeClock();
  const calls: string[] = [];
  const rebootModes: RebootMode[] = [];
  const next = <T>(name: string, answers: Result<T>[]): Result<T> => {
    calls.push(name);
    const answer = answers.length > 1 ? answers.shift() : answers[0];
    return answer ?? fail({ kind: "network" });
  };
  const flow = createRebootFlow({
    api: {
      reboot: (mode) => {
        calls.push("reboot");
        rebootModes.push(mode);
        return Promise.resolve(script.reboot);
      },
      status: () => Promise.resolve(next("status", script.status)),
      system: () => Promise.resolve(next("system", script.system)),
    },
    sleep: (ms) =>
      new Promise((done) => {
        clock.setTimeout(done, ms);
      }),
    now: () => clock.now,
  });
  const seen: RebootProgress["kind"][] = [];
  flow.progress.subscribe((progress) => {
    if (seen[seen.length - 1] !== progress.kind) seen.push(progress.kind);
  });
  return { clock, calls, flow, rebootModes, seen };
}

test("a reboot is requested, followed down and up, and ends when the uptime is lower than before", async () => {
  const { clock, flow, seen, rebootModes } = setup({
    reboot: ok(undefined),
    status: [ok(pending), ok(pending), fail({ kind: "network" })],
    system: [ok(system(1_001)), ok(system(1_003)), fail({ kind: "timeout", afterMs: 8_000 }), ok(system(4))],
  });
  const done = flow.start("graceful", 1_000);
  await settle();
  assert.deepEqual(rebootModes, ["graceful"]);
  assert.deepEqual(flow.progress.value, { kind: "waiting", status: pending, fetchedAt: 0 });

  await clock.advance(POLL_INTERVAL_MS * 6);
  await done;
  assert.deepEqual(flow.progress.value, { kind: "restarted" });
  assert.deepEqual(seen, ["idle", "requesting", "waiting", "restarting", "restarted"]);
});

test("losing the connection while asking is an unknown outcome and the flow carries on", async () => {
  const { clock, flow } = setup({
    reboot: fail({ kind: "unknown-outcome", cause: { kind: "network" } }),
    status: [fail({ kind: "network" })],
    system: [fail({ kind: "network" }), ok(system(2))],
  });
  const done = flow.start("immediate", 500);
  await clock.advance(POLL_INTERVAL_MS * 3);
  await done;
  assert.deepEqual(flow.progress.value, { kind: "restarted" });
});

test("a refusal ends the flow with the failure", async () => {
  const refused = fail<undefined>({ kind: "api", status: 400, code: "bad_request" });
  const { calls, flow } = setup({ reboot: refused, status: [ok(idle)], system: [ok(system(5))] });
  await flow.start("graceful", 1_000);
  assert.deepEqual(calls, ["reboot"]);
  assert.equal(flow.progress.value.kind, "failed");
  if (flow.progress.value.kind === "failed") {
    assert.deepEqual(flow.progress.value.failure, { kind: "api", status: 400, code: "bad_request" });
  }
});

test("a device that doesn't come back within two minutes is reported", async () => {
  const { clock, flow } = setup({
    reboot: ok(undefined),
    status: [fail({ kind: "network" })],
    system: [fail({ kind: "network" })],
  });
  const done = flow.start("graceful", 1_000);
  await clock.advance(RESTART_TIMEOUT_MS - POLL_INTERVAL_MS);
  assert.equal(flow.progress.value.kind, "restarting");
  await clock.advance(POLL_INTERVAL_MS * 2);
  await done;
  assert.deepEqual(flow.progress.value, { kind: "timed-out" });
});

test("a device that answers with a higher uptime hasn't restarted yet", async () => {
  const { clock, flow } = setup({
    reboot: ok(undefined),
    status: [ok(pending)],
    system: [ok(system(2_000))],
  });
  const done = flow.start("graceful", 1_000);
  await clock.advance(RESTART_TIMEOUT_MS + POLL_INTERVAL_MS);
  await done;
  assert.deepEqual(flow.progress.value, { kind: "timed-out" });
});

test("a second start while one runs is ignored, and reset waits for the end", async () => {
  const { clock, calls, flow } = setup({
    reboot: ok(undefined),
    status: [ok(pending)],
    system: [ok(system(1_500)), ok(system(1))],
  });
  const first = flow.start("graceful", 1_000);
  const second = flow.start("immediate", 1_000);
  await settle();
  flow.reset();
  await clock.advance(POLL_INTERVAL_MS);
  await Promise.all([first, second]);
  assert.equal(calls.filter((call) => call === "reboot").length, 1);
  assert.equal(flow.progress.value.kind, "restarted");
  flow.reset();
  assert.equal(flow.progress.value.kind, "idle");
});
