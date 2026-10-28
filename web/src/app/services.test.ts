import assert from "node:assert/strict";
import { test } from "node:test";
import { FakeClock, FakeFetch, emptyResponse, jsonResponse, settle } from "../testing/fakes.ts";
import { createServices } from "./services.ts";

function setup() {
  const clock = new FakeClock();
  const fake = new FakeFetch();
  let hash = "#/system";
  const hashListeners = new Set<() => void>();
  const services = createServices({
    fetch: fake.fetch,
    timers: clock,
    random: () => 0.5,
    now: () => clock.now,
    activity: {
      visible: () => true,
      onVisibilityChange: () => () => {},
      onInput: () => () => {},
      timers: clock,
    },
    router: {
      hash: () => hash,
      setHash(next) {
        hash = next;
        hashListeners.forEach((listener) => listener());
      },
      onHashChange(listener) {
        hashListeners.add(listener);
        return () => hashListeners.delete(listener);
      },
    },
  });
  return { clock, fake, services };
}

test("the services start on the route in the URL", () => {
  const { services } = setup();
  assert.deepEqual(services.router.route.value, { name: "system" });
});

test("a call through the client reaches the device under /api/v1", async () => {
  const { fake, services } = setup();
  fake.reply(() => jsonResponse(200, { id: "4WSG-0KD7-9C6", name: "eveningstar-0kd7-9c6", uptime_s: 12 }));
  const result = await services.api.request("GetSystem");
  assert.equal(result.ok && result.value.uptime_s, 12);
  assert.equal(fake.calls[0]?.url, "/api/v1/system");
  assert.equal(services.connection.state.value, "online");
});

test("failed requests move the connection to degraded and then offline", async () => {
  const { fake, services } = setup();
  fake.reply(FakeFetch.fail, FakeFetch.fail, FakeFetch.fail);
  await services.api.request("GetSystem", { priority: "poll" });
  assert.equal(services.connection.state.value, "degraded");
  await services.api.request("GetSystem", { priority: "poll" });
  await services.api.request("GetSystem", { priority: "poll" });
  assert.equal(services.connection.state.value, "offline");
});

test("an error answer means the device is reachable", async () => {
  const { fake, services } = setup();
  fake.reply(FakeFetch.fail, () => jsonResponse(404, { error: "not_found" }));
  await services.api.request("GetSystem", { priority: "poll" });
  assert.equal(services.connection.state.value, "degraded");
  await services.api.request("GetSystem", { priority: "poll" });
  assert.equal(services.connection.state.value, "online");
});

test("a 401 sends the person to the login route", async () => {
  const { fake, services } = setup();
  fake.reply(() => emptyResponse(401));
  void services.api.request("GetSystem");
  await settle();
  assert.deepEqual(services.router.route.value, { name: "login" });
  assert.equal(services.transport.paused, true);
});

test("the reboot request is sent as a post with its mode", async () => {
  const { fake, services } = setup();
  fake.reply(() => emptyResponse(202));
  const result = await services.api.request("Reboot", { priority: "action", body: { mode: "immediate" } });
  assert.equal(result.ok, true);
  assert.equal(fake.calls[0]?.init.method, "POST");
  assert.equal(fake.calls[0]?.init.body, '{"mode":"immediate"}');
});

test("an answer with a body where none is expected is a decode failure", async () => {
  const { fake, services } = setup();
  fake.reply(() => jsonResponse(202, { surprise: true }));
  const result = await services.api.request("Reboot", { priority: "action", body: { mode: "graceful" } });
  assert.deepEqual(!result.ok && result.failure, { kind: "decode", status: 202 });
});
