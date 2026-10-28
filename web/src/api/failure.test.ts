import assert from "node:assert/strict";
import { test } from "node:test";
import { describeFailure, failureDetails, isTransient } from "./failure.ts";
import type { ApiFailure } from "./failure.ts";

const failures: ApiFailure[] = [
  { kind: "network" },
  { kind: "timeout", afterMs: 8_000 },
  { kind: "busy" },
  { kind: "busy", retryAfterS: 3 },
  { kind: "unauthorized" },
  { kind: "forbidden" },
  { kind: "locked" },
  { kind: "locked", holder: "operator" },
  { kind: "api", status: 400, code: "bad_request" },
  { kind: "api", status: 404, code: "not_found" },
  { kind: "api", status: 405, code: "method_not_allowed" },
  { kind: "api", status: 413, code: "payload_too_large" },
  { kind: "api", status: 400, code: "from_the_future" },
  { kind: "decode", status: 200 },
  { kind: "aborted" },
  { kind: "unknown-outcome", cause: { kind: "network" } },
];

test("every failure has a title, an explanation and a way forward", () => {
  for (const failure of failures) {
    const message = describeFailure(failure);
    assert.ok(message.title.length > 0, JSON.stringify(failure));
    assert.ok(message.explanation.length > 20, JSON.stringify(failure));
    assert.ok(message.title !== message.explanation);
  }
});

test("a network failure says what may be wrong, and offers a retry", () => {
  const message = describeFailure({ kind: "network" });
  assert.equal(message.title, "Can't reach the device");
  assert.match(message.explanation, /restarting/);
  assert.deepEqual(message.actions, ["retry"]);
});

test("an error code from a newer firmware is named and suggests a reload", () => {
  const message = describeFailure({ kind: "api", status: 400, code: "from_the_future" });
  assert.match(message.explanation, /from_the_future/);
  assert.deepEqual(message.actions, ["reload"]);
});

test("an unknown outcome says the request wasn't repeated", () => {
  const message = describeFailure({ kind: "unknown-outcome", cause: { kind: "timeout", afterMs: 10_000 } });
  assert.match(message.explanation, /wasn't repeated/);
  assert.match(message.explanation, /in time/);
});

test("only failures of the connection are transient", () => {
  const transient = failures.filter(isTransient).map((failure) => failure.kind);
  assert.deepEqual(transient, ["network", "timeout", "busy", "busy"]);
});

test("the details give the request, the answer and the attempts", () => {
  const lines = failureDetails(
    { kind: "api", status: 404, code: "not_found" },
    { method: "GET", path: "/api/v1/system", attempts: 1, at: Date.UTC(2026, 9, 28, 12) },
  );
  assert.deepEqual(lines, [
    "GET /api/v1/system",
    "status 404",
    "code not_found",
    "kind api",
    "attempts 1",
    "2026-10-28T12:00:00.000Z",
  ]);
});

test("the details of an unknown outcome include its cause", () => {
  const lines = failureDetails(
    { kind: "unknown-outcome", cause: { kind: "timeout", afterMs: 10_000 } },
    { method: "POST", path: "/api/v1/system/reboot", attempts: 1, at: 0 },
  );
  assert.ok(lines.includes("timed out after 10000 ms"));
  assert.ok(lines.includes("kind unknown-outcome"));
});
