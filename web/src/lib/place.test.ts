import assert from "node:assert/strict";
import { test } from "node:test";
import { placePopover, stepIndex } from "./place.ts";

const viewport = { width: 1280, height: 800 };
const size = { width: 360, height: 120 };

test("a popover sits 6 px under its trigger, aligned to its left edge", () => {
  assert.deepEqual(placePopover({ left: 200, top: 100, width: 18, height: 18 }, size, viewport), { left: 200, top: 124 });
});

test("a popover keeps 16 px from the viewport's sides", () => {
  assert.equal(placePopover({ left: 1200, top: 100, width: 18, height: 18 }, size, viewport).left, 1280 - 360 - 16);
  assert.equal(placePopover({ left: 4, top: 100, width: 18, height: 18 }, size, viewport).left, 16);
});

test("a popover that doesn't fit below its trigger goes above it", () => {
  assert.equal(placePopover({ left: 200, top: 740, width: 18, height: 18 }, size, viewport).top, 740 - 120 - 6);
});

test("a popover that fits neither below nor above keeps 8 px from the bottom", () => {
  const tall = { width: 360, height: 700 };
  assert.equal(placePopover({ left: 200, top: 300, width: 18, height: 18 }, tall, viewport).top, 800 - 700 - 8);
  assert.equal(placePopover({ left: 200, top: 300, width: 18, height: 18 }, { width: 360, height: 900 }, viewport).top, 8);
});

test("Up and Down step through a list and stop at its ends", () => {
  assert.equal(stepIndex(0, 3, "ArrowDown"), 1);
  assert.equal(stepIndex(2, 3, "ArrowDown"), undefined);
  assert.equal(stepIndex(1, 3, "ArrowUp"), 0);
  assert.equal(stepIndex(0, 3, "ArrowUp"), undefined);
  assert.equal(stepIndex(1, 3, "Enter"), undefined);
});
