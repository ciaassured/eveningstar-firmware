import assert from "node:assert/strict";
import { test } from "node:test";
import { FakeClock, FakeFetch, emptyResponse, jsonResponse, serveContent, settle } from "../testing/fakes.ts";
import { appImage, internalFlash, regions } from "../testing/storage.ts";
import { createServices } from "./services.ts";

function setup() {
  const clock = new FakeClock();
  const fake = new FakeFetch();
  let hash = "#/system";
  const hashListeners = new Set<() => void>();
  const saved: { name: string; size: number }[] = [];
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
    downloads: { save: (bytes, name) => saved.push({ name, size: bytes.byteLength }), sha256: undefined, guardUnload: () => {} },
    clipboard: undefined,
  });
  return { clock, fake, services, saved };
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

test("a call can ask the transport not to retry", async () => {
  const { clock, fake, services } = setup();
  fake.reply(FakeFetch.fail, FakeFetch.fail);
  const result = await services.api.request("GetSystem", { priority: "view", retry: false });
  await clock.advance(10_000);
  assert.equal(fake.calls.length, 1);
  assert.equal(!result.ok && result.failure.kind, "network");
});

test("a download plans against the flash read when it starts, names the file after the device, and saves it", async () => {
  const { clock, fake, services, saved } = setup();
  const image = appImage();
  const slot = new Uint8Array(0x30_0000).fill(0xff);
  slot.set(image.bytes);
  fake.reply(
    () => jsonResponse(200, internalFlash("full", image)),
    () => jsonResponse(200, { regions: regions() }),
    () => jsonResponse(200, { id: "4WSG-0KD7-9C6", name: "eveningstar-0kd7-9c6", uptime_s: 12 }),
    serveContent(slot, { etag: '"a-0"' }),
  );
  services.downloads.add({ region: "app_current", what: { kind: "image" } });
  await clock.advance(1);
  assert.deepEqual(
    fake.calls.map((call) => call.url),
    [
      "/api/v1/storage/internal",
      "/api/v1/storage/internal/regions",
      "/api/v1/system",
      "/api/v1/storage/internal/regions/app_current/content",
    ],
  );
  assert.deepEqual(saved, [
    { name: `eveningstar-0kd7-9c6-app_current-ota_0-image-${image.fileSha256.slice(0, 8)}.bin`, size: 4_096 },
  ]);
  assert.equal(services.storage.flash.value?.tier, "full");
});

test("without that, a view's call is retried", async () => {
  const { clock, fake, services } = setup();
  fake.reply(FakeFetch.fail, () => jsonResponse(200, { id: "4WSG-0KD7-9C6", name: "n", uptime_s: 1 }));
  const result = services.api.request("GetSystem", { priority: "view" });
  await clock.advance(1_000);
  assert.equal((await result).ok, true);
  assert.equal(fake.calls.length, 2);
});
