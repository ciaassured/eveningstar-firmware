import assert from "node:assert/strict";
import { test } from "node:test";
import { copyOrSelect } from "./clipboard.ts";

function run(write: ((text: string) => Promise<void>) | undefined) {
  const calls: string[] = [];
  const outcome = copyOrSelect(
    "abc",
    write === undefined
      ? undefined
      : (text) => {
          calls.push(`write ${text}`);
          return write(text);
        },
    () => {
      calls.push("select");
    },
  );
  return { calls, outcome };
}

test("copying writes the text, and selects nothing when the clipboard takes it", async () => {
  const { calls, outcome } = run(() => Promise.resolve());
  // The write happens before the handler returns, while the click still counts as the person's action.
  assert.deepEqual(calls, ["write abc"]);
  assert.equal(await outcome, "copied");
  assert.deepEqual(calls, ["write abc"]);
});

test("copying selects the text without a clipboard, or when the clipboard refuses or throws", async () => {
  const none = run(undefined);
  assert.deepEqual(none.calls, ["select"]);
  assert.equal(await none.outcome, "selected");

  const refused = run(() => Promise.reject(new Error("denied")));
  assert.equal(await refused.outcome, "selected");
  assert.deepEqual(refused.calls, ["write abc", "select"]);

  const thrown = run(() => {
    throw new Error("not allowed");
  });
  assert.deepEqual(thrown.calls, ["write abc", "select"]);
  assert.equal(await thrown.outcome, "selected");
});
