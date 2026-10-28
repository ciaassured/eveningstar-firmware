import assert from "node:assert/strict";
import { test } from "node:test";
import { rebootHolderLabel, rebootModeLabel, rebootReasonLabel } from "./labels.ts";

test("known reasons, holders and modes read as words", () => {
  assert.equal(rebootReasonLabel("link_down"), "The Ethernet link stayed down");
  assert.equal(rebootReasonLabel("api"), "Requested over the API");
  assert.equal(rebootHolderLabel("http"), "A web request in progress");
  assert.equal(rebootModeLabel("graceful"), "Wait for work in progress to finish");
  assert.equal(rebootModeLabel("immediate"), "Restart now");
});

test("a value from newer firmware is shown as it came", () => {
  assert.equal(rebootReasonLabel("thermal"), 'Reason "thermal"');
  assert.equal(rebootHolderLabel("modbus"), 'Holder "modbus"');
});
