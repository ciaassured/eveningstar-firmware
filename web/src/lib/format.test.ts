import assert from "node:assert/strict";
import { test } from "node:test";
import { backoffDelayMs, widen } from "./backoff.ts";
import {
  formatAge,
  formatByteCount,
  formatCountdown,
  formatDecimal,
  formatHex,
  formatPercent1,
  formatSize,
  formatSizeText,
  formatSpan,
  formatUptime,
  joinWords,
} from "./format.ts";

test("formatSize uses MiB from 1 MiB and KiB from 1 KiB, with up to two decimals", () => {
  const cases: [number, string][] = [
    [0x30_0000, "3 MiB"],
    [0x12_0000, "1.13 MiB"],
    [1_509_949, "1.44 MiB"],
    [22_672, "22.14 KiB"],
    [0x1000, "4 KiB"],
    [0xc00, "3 KiB"],
    [1_024, "1 KiB"],
    [1_023, "1,023 bytes"],
    [32, "32 bytes"],
    [0, "0 bytes"],
  ];
  for (const [bytes, text] of cases) assert.equal(formatSize(bytes), text, `${bytes}`);
});

test("decimals group with commas, and byte counts say bytes", () => {
  assert.equal(formatDecimal(3_211_264), "3,211,264");
  assert.equal(formatDecimal(0), "0");
  assert.equal(formatByteCount(1_152_096), "1,152,096 bytes");
});

test("offsets are six hex digits at least, and wider values keep every digit", () => {
  assert.equal(formatHex(0x1_0000), "0x010000");
  assert.equal(formatHex(0), "0x000000");
  assert.equal(formatHex(0x100_0000), "0x1000000");
  assert.equal(formatHex(0xc00, 0), "0xc00");
  assert.equal(formatHex(1, 2), "0x01");
});

test("a span reads as its start, its exclusive end and its size, in hex and decimal", () => {
  assert.deepEqual(formatSpan(0x9000, 0x1000), {
    start: { hex: "0x009000", decimal: "36,864" },
    end: { hex: "0x00a000", decimal: "40,960" },
    size: { hex: "0x001000", text: "4,096 bytes, 4 KiB", human: "4 KiB" },
  });
  assert.deepEqual(formatSizeText(0x30_0000), { hex: "0x300000", text: "3,145,728 bytes, 3 MiB", human: "3 MiB" });
  assert.deepEqual(formatSizeText(32), { hex: "0x000020", text: "32 bytes", human: "32 bytes" });
});

test("percentages have one decimal, and words join as a list", () => {
  assert.equal(formatPercent1(0.376), "37.6%");
  assert.equal(formatPercent1(1), "100.0%");
  assert.equal(formatPercent1(0.0013), "0.1%");
  assert.equal(joinWords([]), "");
  assert.equal(joinWords(["offset"]), "offset");
  assert.equal(joinWords(["offset", "size"]), "offset and size");
  assert.equal(joinWords(["type", "offset", "size"]), "type, offset and size");
});

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
