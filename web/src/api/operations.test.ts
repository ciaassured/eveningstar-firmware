import assert from "node:assert/strict";
import { test } from "node:test";
import { CAPABILITIES } from "./capability.ts";
import { OPERATION_CAPABILITY } from "./generated/capabilities.ts";
import { OPERATIONS } from "./operations.ts";

test("every operation's capability is the one the API declares", () => {
  for (const [id, meta] of Object.entries(OPERATIONS)) {
    assert.equal(meta.capability, OPERATION_CAPABILITY[id as keyof typeof OPERATIONS], id);
  }
});

test("reading needs read and a reboot needs operate", () => {
  assert.equal(OPERATIONS.GetSystem.capability, "read");
  assert.equal(OPERATIONS.GetRebootStatus.capability, "read");
  assert.equal(OPERATIONS.Reboot.capability, "operate");
});

test("the capabilities are the API's three", () => {
  assert.deepEqual([...CAPABILITIES].sort(), ["configure", "operate", "read"]);
});
