import assert from "node:assert/strict";
import { test } from "node:test";
import { SCOPES } from "./scope.ts";
import { OPERATION_SCOPE } from "./generated/scopes.ts";
import { OPERATIONS, expandPath, operationUrl } from "./operations.ts";

test("every operation's scope is the one the API declares", () => {
  for (const [id, meta] of Object.entries(OPERATIONS)) {
    assert.equal(meta.scope, OPERATION_SCOPE[id as keyof typeof OPERATIONS], id);
  }
});

test("reading needs read and a reboot needs operate", () => {
  assert.equal(OPERATIONS.GetSystem.scope, "read");
  assert.equal(OPERATIONS.GetRebootStatus.scope, "read");
  assert.equal(OPERATIONS.Reboot.scope, "operate");
  assert.equal(OPERATIONS.GetStorage.scope, "read");
  assert.equal(OPERATIONS.GetLayout.scope, "read");
  assert.equal(OPERATIONS.GetRegions.scope, "read");
  assert.equal(OPERATIONS.GetRegion.scope, "read");
  assert.equal(OPERATIONS.GetView.scope, "read");
});

test("the scopes are the API's three", () => {
  assert.deepEqual([...SCOPES].sort(), ["configure", "operate", "read"]);
});

test("a path's parameters are put in its template, percent-encoded", () => {
  assert.equal(expandPath("/system"), "/system");
  assert.equal(
    expandPath(OPERATIONS.GetRegion.path, { region: "app_current" }),
    "/storage/regions/app_current",
  );
  assert.equal(expandPath("/a/{b}/c/{d}", { b: "x/y", d: "?" }), "/a/x%2Fy/c/%3F");
  assert.throws(() => expandPath(OPERATIONS.GetRegion.path));
});

test("a view's URL names its region and view", () => {
  assert.equal(
    operationUrl("GetView", { region: "otadata", view: "entry_0" }),
    "/api/v1/storage/regions/otadata/views/entry_0",
  );
});
