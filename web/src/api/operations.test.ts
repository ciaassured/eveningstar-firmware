import assert from "node:assert/strict";
import { test } from "node:test";
import { SCOPES } from "./scope.ts";
import { OPERATION_SCOPE } from "./generated/scopes.ts";
import { OPERATIONS } from "./operations.ts";

test("every operation's scope is the one the API declares", () => {
  for (const [id, meta] of Object.entries(OPERATIONS)) {
    assert.equal(meta.scope, OPERATION_SCOPE[id as keyof typeof OPERATIONS], id);
  }
});

test("reading needs read and a reboot needs operate", () => {
  assert.equal(OPERATIONS.GetSystem.scope, "read");
  assert.equal(OPERATIONS.GetRebootStatus.scope, "read");
  assert.equal(OPERATIONS.Reboot.scope, "operate");
});

test("the scopes are the API's three", () => {
  assert.deepEqual([...SCOPES].sort(), ["configure", "operate", "read"]);
});
