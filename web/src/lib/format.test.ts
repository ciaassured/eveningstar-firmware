import assert from "node:assert/strict";
import { test } from "node:test";
import { backoffDelayMs, widen } from "./backoff.ts";
import { formatAge, formatCountdown, formatUptime } from "./format.ts";

test("formatUptime picks the two largest units", () => {
  assert.equal(formatUptime(9), "9s");
  assert.equal(formatUptime(59), "59s");
  assert.equal(formatUptime(125), "2m 5s");
  assert.equal(formatUptime(3_660), "1h 1m");
  assert.equal(formatUptime(90_000), "1d 1h");
});

test("formatCountdown pads seconds", () => {
  assert.equal(formatCountdown(0), "0:00");
  assert.equal(formatCountdown(9.2), "0:10");
  assert.equal(formatCountdown(125), "2:05");
  assert.equal(formatCountdown(-3), "0:00");
});

test("formatAge steps through units", () => {
  assert.equal(formatAge(1_000, "en"), "now");
  assert.equal(formatAge(12_000, "en"), "12 seconds ago");
  assert.equal(formatAge(120_000, "en"), "2 minutes ago");
  assert.equal(formatAge(3 * 3_600_000, "en"), "3 hours ago");
  assert.equal(formatAge(-5, "en"), "now");
});

test("backoffDelayMs doubles, repeats the last delay and jitters by a quarter", () => {
  const middle = () => 0.5;
  assert.deepEqual(
    [0, 1, 2, 3, 4, 9].map((retry) => backoffDelayMs(retry, middle)),
    [500, 1_000, 2_000, 4_000, 4_000, 4_000],
  );
  assert.equal(backoffDelayMs(1, () => 0), 750);
  assert.equal(backoffDelayMs(1, () => 0.999999), 1_250);
});

test("widen doubles up to the maximum", () => {
  assert.equal(widen(5_000, 30_000), 10_000);
  assert.equal(widen(20_000, 30_000), 30_000);
});
