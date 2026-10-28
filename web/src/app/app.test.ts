import assert from "node:assert/strict";
import { test } from "node:test";
import type { Result } from "../api/client.ts";
import type { FailureContext } from "../api/failure.ts";
import { FakeClock, settle } from "../testing/fakes.ts";
import { createLoadController } from "./load-controller.ts";
import { createRouter, parseRoute } from "./router.ts";
import type { RouterEnvironment } from "./router.ts";

test("parseRoute names the routes and tolerates a trailing slash and a query", () => {
  assert.deepEqual(parseRoute(""), { name: "dashboard" });
  assert.deepEqual(parseRoute("#"), { name: "dashboard" });
  assert.deepEqual(parseRoute("#/"), { name: "dashboard" });
  assert.deepEqual(parseRoute("#/system"), { name: "system" });
  assert.deepEqual(parseRoute("#/system/"), { name: "system" });
  assert.deepEqual(parseRoute("#/system?tab=1"), { name: "system" });
  assert.deepEqual(parseRoute("#/login"), { name: "login" });
  assert.deepEqual(parseRoute("#/nope"), { name: "not-found", path: "/nope" });
});

function fakeBrowser(initial = "") {
  let hash = initial;
  const listeners = new Set<() => void>();
  const environment: RouterEnvironment = {
    hash: () => hash,
    setHash(next) {
      if (next === hash) return;
      hash = next;
      listeners.forEach((listener) => listener());
    },
    onHashChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  return { environment, hash: () => hash, listeners };
}

test("the router follows the fragment", () => {
  const browser = fakeBrowser("#/system");
  const router = createRouter(browser.environment);
  assert.deepEqual(router.route.value, { name: "system" });
  router.navigate("/");
  assert.deepEqual(router.route.value, { name: "dashboard" });
  router.dispose();
  assert.equal(browser.listeners.size, 0);
});

test("a guard can keep the person on the page", () => {
  const browser = fakeBrowser("#/system");
  const router = createRouter(browser.environment);
  router.setGuard((to) => to.name === "system");
  router.navigate("/");
  assert.deepEqual(router.route.value, { name: "system" });
  assert.equal(browser.hash(), "#/system");
  router.setGuard(undefined);
  router.navigate("/");
  assert.deepEqual(router.route.value, { name: "dashboard" });
});

test("login remembers the route and goes back to it", () => {
  const browser = fakeBrowser("#/system");
  const router = createRouter(browser.environment);
  router.requireLogin();
  assert.deepEqual(router.route.value, { name: "login" });
  router.requireLogin();
  router.returnFromLogin();
  assert.deepEqual(router.route.value, { name: "system" });
});

const context: FailureContext = { method: "GET", path: "/x", attempts: 1, at: 0 };
const ok: Result<unknown> = { ok: true, value: 1, context };

test("a view is ready only when every loader succeeded, in order", async () => {
  const clock = new FakeClock();
  const order: string[] = [];
  const controller = createLoadController({
    loaders: [
      () => {
        order.push("a");
        return Promise.resolve(ok);
      },
      () => {
        order.push("b");
        return Promise.resolve(ok);
      },
    ],
    timers: clock,
    abort: () => {},
  });
  assert.equal(controller.state.value.kind, "loading");
  await controller.load();
  assert.deepEqual(order, ["a", "b"]);
  assert.equal(controller.state.value.kind, "ready");
});

test("a failed loader shows its failure, and the later loaders don't run", async () => {
  const clock = new FakeClock();
  let ran = false;
  const controller = createLoadController({
    loaders: [
      () => Promise.resolve({ ok: false, failure: { kind: "api", status: 404, code: "not_found" }, context }),
      () => {
        ran = true;
        return Promise.resolve(ok);
      },
    ],
    timers: clock,
    abort: () => {},
  });
  await controller.load();
  assert.equal(ran, false);
  const state = controller.state.value;
  assert.equal(state.kind, "failed");
  assert.equal(state.kind === "failed" && state.retryInS, undefined);
  assert.equal(clock.waiting, 0);
});

test("a transient failure counts down and tries again", async () => {
  const clock = new FakeClock();
  const answers: Result<unknown>[] = [
    { ok: false, failure: { kind: "network" }, context },
    { ok: false, failure: { kind: "network" }, context },
    ok,
  ];
  let loads = 0;
  const controller = createLoadController({
    loaders: [
      () => {
        loads += 1;
        return Promise.resolve(answers.shift() ?? ok);
      },
    ],
    timers: clock,
    abort: () => {},
  });
  await controller.load();
  let state = controller.state.value;
  assert.equal(state.kind === "failed" && state.retryInS, 3);
  await clock.advance(1_000);
  state = controller.state.value;
  assert.equal(state.kind === "failed" && state.retryInS, 2);
  await clock.advance(2_000);
  assert.equal(loads, 2);
  state = controller.state.value;
  assert.equal(state.kind === "failed" && state.retryInS, 6);
  await clock.advance(6_000);
  assert.equal(controller.state.value.kind, "ready");
});

test("retry loads again at once, and dispose stops the countdown and aborts the view's requests", async () => {
  const clock = new FakeClock();
  let loads = 0;
  let aborted = 0;
  const controller = createLoadController({
    loaders: [
      () => {
        loads += 1;
        return Promise.resolve({ ok: false, failure: { kind: "timeout", afterMs: 8_000 }, context });
      },
    ],
    timers: clock,
    abort: () => {
      aborted += 1;
    },
  });
  await controller.load();
  controller.retry();
  await settle();
  assert.equal(loads, 2);
  controller.dispose();
  assert.equal(aborted, 1);
  await clock.advance(60_000);
  assert.equal(loads, 2);
});

test("an aborted load shows nothing", async () => {
  const clock = new FakeClock();
  const controller = createLoadController({
    loaders: [() => Promise.resolve({ ok: false, failure: { kind: "aborted" }, context })],
    timers: clock,
    abort: () => {},
  });
  await controller.load();
  assert.equal(controller.state.value.kind, "loading");
});
