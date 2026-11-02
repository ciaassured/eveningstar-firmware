import assert from "node:assert/strict";
import { test } from "node:test";
import { FakeClock } from "../testing/fakes.ts";
import { editEntries, storageInput } from "../testing/storage.ts";
import { buildModel } from "./items.ts";
import { ADDED_MS, createStoragePage } from "./page.ts";

function setup() {
  const clock = new FakeClock();
  const frames: (() => void)[] = [];
  const page = createStoragePage({
    timers: clock,
    frame: (callback) => {
      frames.push(callback);
    },
  });
  /** Draws the next frame: runs the callbacks asked for before it. */
  const draw = (): void => {
    for (const callback of frames.splice(0)) callback();
  };
  return { clock, page, draw, frames };
}

test("the page selects the running app's region first, and keeps the selection across a refresh by its key", () => {
  const { page } = setup();
  page.sync(buildModel(storageInput()));
  assert.equal(page.selected.value, "label:ota_0");
  page.select("label:otadata", false);
  page.sync(buildModel(storageInput()));
  assert.equal(page.selected.value, "label:otadata");
});

test("a selection whose item is gone after a refresh falls back to the running app's region", () => {
  const { page } = setup();
  page.sync(buildModel(storageInput()));
  page.select("gap:49152", false);
  page.sync(buildModel(storageInput({ entries: editEntries({}, [{ label: "scratch", type: 1, subtype: 0x40, offset: 0xc000, size: 0x4000, flags: 0 }]) })));
  assert.equal(page.selected.value, "label:ota_0");
});

test("on a phone the inspector is drawn closed with the new item, then slides in two frames later", () => {
  const { page, draw } = setup();
  page.select("label:otadata", true);
  assert.equal(page.selected.value, "label:otadata");
  assert.equal(page.slide.value, "opening");
  draw();
  assert.equal(page.slide.value, "opening");
  draw();
  assert.equal(page.slide.value, "open");
});

test("on a phone, selecting with the inspector open keeps it open, and going back closes it", () => {
  const { page, draw, frames } = setup();
  page.select("label:otadata", true);
  draw();
  draw();
  page.select("bootloader", true);
  assert.equal(page.slide.value, "open");
  assert.equal(frames.length, 0);
  page.back();
  assert.equal(page.slide.value, "closed");
  assert.equal(page.selected.value, "bootloader");
});

test("going back while the inspector is about to slide in leaves it closed", () => {
  const { page, draw } = setup();
  page.select("label:otadata", true);
  page.back();
  draw();
  draw();
  assert.equal(page.slide.value, "closed");
});

test("a selection on a desktop opens the inspector, so that a phone width keeps showing it", () => {
  const { page, frames } = setup();
  page.select("label:otadata", false);
  assert.equal(page.slide.value, "open");
  assert.equal(frames.length, 0);
});

test("a download button reads as added for a while after each click", async () => {
  const { clock, page } = setup();
  page.markAdded("label:ota_0:image");
  assert.equal(page.added.value, "label:ota_0:image");
  await clock.advance(ADDED_MS - 100);
  page.markAdded("label:ota_0:whole");
  await clock.advance(ADDED_MS - 100);
  assert.equal(page.added.value, "label:ota_0:whole");
  await clock.advance(100);
  assert.equal(page.added.value, undefined);
});
