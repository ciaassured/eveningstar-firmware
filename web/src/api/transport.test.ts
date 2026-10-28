import assert from "node:assert/strict";
import { test } from "node:test";
import { FakeClock, FakeFetch, emptyResponse, jsonResponse, settle } from "../testing/fakes.ts";
import type { AuthProvider } from "./auth.ts";
import { Transport } from "./transport.ts";
import type { RequestSpec, TransportDeps } from "./transport.ts";

const middle = () => 0.5;

function setup(auth?: Partial<AuthProvider>, onLoginRequired: () => void = () => {}) {
  const clock = new FakeClock();
  const fake = new FakeFetch();
  const deps: TransportDeps = {
    fetch: fake.fetch,
    timers: clock,
    random: middle,
    now: () => clock.now,
    auth: {
      decorate: (init) => init,
      onUnauthorized: () => Promise.resolve("login-required"),
      ...auth,
    },
    onLoginRequired,
  };
  return { clock, fake, transport: new Transport(deps) };
}

const get = (path: string, extra: Partial<RequestSpec> = {}): RequestSpec => ({
  method: "GET",
  path,
  idempotent: true,
  priority: "view",
  ...extra,
});

const post = (path: string, extra: Partial<RequestSpec> = {}): RequestSpec => ({
  method: "POST",
  path,
  body: { mode: "graceful" },
  idempotent: false,
  priority: "action",
  ...extra,
});

test("rule 1: one request is in flight, and the most urgent waits first", async () => {
  const { fake, transport } = setup();
  const first = FakeFetch.deferred(jsonResponse(200, { n: 1 }));
  fake.reply(
    first.step,
    () => jsonResponse(200, { n: 2 }),
    () => jsonResponse(200, { n: 3 }),
    () => jsonResponse(200, { n: 4 }),
  );

  const running = transport.send(get("/running"));
  const poll = transport.send(get("/poll", { priority: "poll" }));
  const view = transport.send(get("/view"));
  const action = transport.send(post("/action"));
  await settle();
  assert.equal(fake.calls.length, 1);

  first.release();
  await Promise.all([running, poll, view, action]);
  assert.deepEqual(
    fake.calls.map((call) => call.url),
    ["/running", "/action", "/view", "/poll"],
  );
  assert.equal(fake.maxInFlight, 1);
});

test("rule 2: polls of one resource coalesce", async () => {
  const { fake, transport } = setup();
  const first = FakeFetch.deferred(jsonResponse(200, { ok: true }));
  fake.reply(first.step);

  const a = transport.send(get("/system", { priority: "poll", resource: "GetSystem" }));
  const b = transport.send(get("/system", { priority: "poll", resource: "GetSystem" }));
  await settle();
  first.release();
  const [outcomeA, outcomeB] = await Promise.all([a, b]);
  assert.equal(fake.calls.length, 1);
  assert.equal(outcomeA, outcomeB);
});

test("rule 2: polls of different resources, and a poll after the first finished, are sent", async () => {
  const { fake, transport } = setup();
  fake.reply(
    () => jsonResponse(200, {}),
    () => jsonResponse(200, {}),
    () => jsonResponse(200, {}),
  );
  await transport.send(get("/a", { priority: "poll", resource: "a" }));
  await transport.send(get("/a", { priority: "poll", resource: "a" }));
  await transport.send(get("/b", { priority: "poll", resource: "b" }));
  assert.equal(fake.calls.length, 3);
});

test("rule 3: requests without a body time out after 8 s, and with a body after 10 s", async () => {
  const { clock, fake, transport } = setup();
  fake.reply(FakeFetch.hang, FakeFetch.hang);

  const read = transport.send(get("/slow", { retry: false }));
  await clock.advance(7_999);
  assert.equal(fake.inFlight, 1);
  await clock.advance(1);
  const readOutcome = await read;
  assert.deepEqual(!readOutcome.ok && readOutcome.failure, { kind: "timeout", afterMs: 8_000 });

  const write = transport.send(post("/slow"));
  await clock.advance(9_999);
  assert.equal(fake.inFlight, 1);
  await clock.advance(1);
  const writeOutcome = await write;
  assert.deepEqual(
    !writeOutcome.ok && writeOutcome.failure,
    { kind: "unknown-outcome", cause: { kind: "timeout", afterMs: 10_000 } },
  );
});

test("rule 3: an operation can override the timeout", async () => {
  const { clock, fake, transport } = setup();
  fake.reply(FakeFetch.hang);
  const read = transport.send(get("/slow", { retry: false, timeoutMs: 1_000 }));
  await clock.advance(1_000);
  const outcome = await read;
  assert.deepEqual(!outcome.ok && outcome.failure, { kind: "timeout", afterMs: 1_000 });
});

test("rule 4: idempotent requests retry after a network error, with backoff", async () => {
  const { clock, fake, transport } = setup();
  fake.reply(FakeFetch.fail, FakeFetch.fail, () => jsonResponse(200, { ok: true }));

  const result = transport.send(get("/system"));
  await settle();
  assert.equal(fake.calls.length, 1);
  await clock.advance(499);
  assert.equal(fake.calls.length, 1);
  await clock.advance(1);
  assert.equal(fake.calls.length, 2);
  await clock.advance(999);
  assert.equal(fake.calls.length, 2);
  await clock.advance(1);
  const outcome = await result;
  assert.equal(fake.calls.length, 3);
  assert.equal(outcome.ok, true);
  assert.equal(outcome.context.attempts, 3);
});

test("rule 4: it gives up after 4 attempts", async () => {
  const { clock, fake, transport } = setup();
  fake.reply(FakeFetch.fail, FakeFetch.fail, FakeFetch.fail, FakeFetch.fail);
  const result = transport.send(get("/system"));
  await clock.advance(10_000);
  const outcome = await result;
  assert.equal(fake.calls.length, 4);
  assert.deepEqual(!outcome.ok && outcome.failure, { kind: "network" });
  assert.equal(outcome.context.attempts, 4);
});

test("rule 4: 503 and 429 retry and honour Retry-After", async () => {
  const { clock, fake, transport } = setup();
  fake.reply(
    () => emptyResponse(503, { "Retry-After": "3" }),
    () => emptyResponse(429),
    () => jsonResponse(200, {}),
  );
  const result = transport.send(get("/system"));
  await clock.advance(2_999);
  assert.equal(fake.calls.length, 1);
  await clock.advance(1);
  assert.equal(fake.calls.length, 2);
  await clock.advance(1_000);
  const outcome = await result;
  assert.equal(fake.calls.length, 3);
  assert.equal(outcome.ok, true);
});

test("rule 4: other 4xx and 5xx answers are not retried", async () => {
  const { fake, transport } = setup();
  fake.reply(
    () => jsonResponse(404, { error: "not_found" }),
    () => jsonResponse(500, { error: "bad_request" }),
  );
  const missing = await transport.send(get("/a"));
  const broken = await transport.send(get("/b"));
  assert.equal(fake.calls.length, 2);
  assert.deepEqual(!missing.ok && missing.failure, { kind: "api", status: 404, code: "not_found" });
  assert.deepEqual(!broken.ok && broken.failure, { kind: "api", status: 500, code: "bad_request" });
});

test("rule 4: polls are not retried by the transport", async () => {
  const { fake, transport } = setup();
  fake.reply(FakeFetch.fail);
  const outcome = await transport.send(get("/system", { priority: "poll" }));
  assert.equal(fake.calls.length, 1);
  assert.equal(!outcome.ok && outcome.failure.kind, "network");
});

test("rule 5: a failed non-idempotent request has an unknown outcome and is not retried", async () => {
  const { clock, fake, transport } = setup();
  fake.reply(FakeFetch.fail);
  const outcome = await transport.send(post("/system/reboot"));
  await clock.advance(30_000);
  assert.equal(fake.calls.length, 1);
  assert.deepEqual(!outcome.ok && outcome.failure, { kind: "unknown-outcome", cause: { kind: "network" } });
});

test("rule 5: a non-idempotent request the device refused is known to have failed", async () => {
  const { fake, transport } = setup();
  fake.reply(() => emptyResponse(503), () => jsonResponse(400, { error: "bad_request" }));
  const busy = await transport.send(post("/a"));
  const bad = await transport.send(post("/b"));
  assert.equal(fake.calls.length, 2);
  assert.deepEqual(!busy.ok && busy.failure, { kind: "busy" });
  assert.deepEqual(!bad.ok && bad.failure, { kind: "api", status: 400, code: "bad_request" });
});

test("rule 6: leaving a page aborts its queued and running view loads", async () => {
  const { fake, transport } = setup();
  fake.reply(FakeFetch.hang, () => jsonResponse(200, { kept: true }));

  const running = transport.send(get("/one", { group: "system" }));
  const queued = transport.send(get("/two", { group: "system" }));
  const other = transport.send(get("/three", { group: "dashboard" }));
  await settle();
  transport.abortGroup("system");
  const [first, second, third] = await Promise.all([running, queued, other]);
  assert.deepEqual(!first.ok && first.failure, { kind: "aborted" });
  assert.deepEqual(!second.ok && second.failure, { kind: "aborted" });
  assert.equal(third.ok, true);
  assert.deepEqual(
    fake.calls.map((call) => call.url),
    ["/one", "/three"],
  );
});

test("rule 6: leaving a page doesn't abort actions or polls", async () => {
  const { fake, transport } = setup();
  const action = FakeFetch.deferred(emptyResponse(202));
  fake.reply(action.step, () => jsonResponse(200, {}));
  const act = transport.send(post("/act", { group: "system" }));
  const poll = transport.send(get("/poll", { priority: "poll", group: "system" }));
  await settle();
  transport.abortGroup("system");
  action.release();
  const [actOutcome, pollOutcome] = await Promise.all([act, poll]);
  assert.equal(actOutcome.ok, true);
  assert.equal(pollOutcome.ok, true);
});

test("rule 7: every result is a value, never a thrown error", async () => {
  const { fake, transport } = setup();
  fake.reply(
    FakeFetch.fail,
    () => new Response("<html>", { status: 200, headers: { "Content-Type": "text/html" } }),
    () => new Response("{", { status: 200, headers: { "Content-Type": "application/json" } }),
    () => jsonResponse(500, { nothing: "useful" }),
  );
  const outcomes = [
    await transport.send(get("/a", { retry: false })),
    await transport.send(get("/b")),
    await transport.send(get("/c")),
    await transport.send(get("/d")),
  ];
  assert.deepEqual(
    outcomes.map((outcome) => !outcome.ok && outcome.failure),
    [{ kind: "network" }, { kind: "decode", status: 200 }, { kind: "decode", status: 200 }, { kind: "decode", status: 500 }],
  );
});

test("requests carry same-origin credentials, JSON headers and the body", async () => {
  const { fake, transport } = setup();
  fake.reply(() => emptyResponse(202));
  await transport.send(post("/system/reboot"));
  const { init } = fake.calls[0] ?? assert.fail("no request");
  assert.equal(init.method, "POST");
  assert.equal(init.credentials, "same-origin");
  assert.equal(init.cache, "no-store");
  assert.equal(init.body, '{"mode":"graceful"}');
  assert.deepEqual(init.headers, { Accept: "application/json", "Content-Type": "application/json" });
});

test("the auth provider decorates every request", async () => {
  const { fake, transport } = setup({
    decorate: (init) => ({ ...init, headers: { ...(init.headers as Record<string, string>), "X-Test": "yes" } }),
  });
  fake.reply(() => jsonResponse(200, {}));
  await transport.send(get("/system"));
  assert.equal((fake.calls[0]?.init.headers as Record<string, string>)["X-Test"], "yes");
});

test("401 with a renewable session retries at once", async () => {
  const { fake, transport } = setup({ onUnauthorized: () => Promise.resolve("retry") });
  fake.reply(() => emptyResponse(401), () => jsonResponse(200, {}));
  const outcome = await transport.send(get("/system"));
  assert.equal(outcome.ok, true);
  assert.equal(fake.calls.length, 2);
});

test("401 pauses the queue; idempotent requests resume after login, others fail", async () => {
  let loginRequired = 0;
  const { fake, transport } = setup(undefined, () => {
    loginRequired += 1;
  });
  fake.reply(() => emptyResponse(401), () => emptyResponse(401), () => jsonResponse(200, { again: true }));

  const read = transport.send(get("/system"));
  await settle();
  assert.equal(transport.paused, true);
  assert.equal(loginRequired, 1);

  const write = transport.send(post("/system/reboot"));
  await settle();
  assert.equal(fake.calls.length, 1);

  transport.resume();
  const writeOutcome = await write;
  assert.deepEqual(!writeOutcome.ok && writeOutcome.failure, { kind: "unauthorized" });
  assert.equal(transport.paused, true);

  transport.resume();
  const readOutcome = await read;
  assert.equal(readOutcome.ok, true);
  assert.equal(fake.calls.length, 3);
});

test("locked answers carry the holder", async () => {
  const { fake, transport } = setup();
  fake.reply(() => jsonResponse(423, { holder: "operator", since: "2026-10-28T10:00:00Z" }));
  const outcome = await transport.send(get("/config"));
  assert.deepEqual(!outcome.ok && outcome.failure, {
    kind: "locked",
    holder: "operator",
    since: "2026-10-28T10:00:00Z",
  });
});
