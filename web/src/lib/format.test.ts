import assert from "node:assert/strict";
import { test } from "node:test";
import { formatUptime } from "./format.ts";

test("formatUptime picks the two largest units", () => {
  assert.equal(formatUptime(59), "0m");
  assert.equal(formatUptime(3_660), "1h 1m");
  assert.equal(formatUptime(90_000), "1d 1h");
});
