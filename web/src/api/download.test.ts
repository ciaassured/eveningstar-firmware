import assert from "node:assert/strict";
import { test } from "node:test";
import { FakeClock, FakeFetch, jsonResponse, serveContent, settle } from "../testing/fakes.ts";
import { createApiClient } from "./client.ts";
import { fillPath } from "./operations.ts";
import { STALL_MS, Transport, classify, parseContentRange } from "./transport.ts";
import type { DownloadHead, TransportDeps } from "./transport.ts";

/** Bytes that aren't all alike, so a misplaced chunk shows. */
function bytesOf(size: number): Uint8Array<ArrayBuffer> {
  return Uint8Array.from({ length: size }, (_, i) => (i * 31 + (i >> 8)) & 0xff);
}

function setup() {
  const clock = new FakeClock();
  const fake = new FakeFetch();
  const attempts: boolean[] = [];
  const deps: TransportDeps = {
    fetch: fake.fetch,
    timers: clock,
    random: () => 0.5,
    now: () => clock.now,
    auth: { decorate: (init) => init, onUnauthorized: () => Promise.resolve("login-required") },
    onLoginRequired: () => {},
    onAttempt: (reachable) => attempts.push(reachable),
  };
  const transport = new Transport(deps);
  return { clock, fake, transport, attempts, api: createApiClient(transport) };
}

function collect() {
  const chunks: Uint8Array[] = [];
  const heads: DownloadHead[] = [];
  return {
    chunks,
    heads,
    onChunk: (chunk: Uint8Array) => chunks.push(chunk),
    onHead: (head: DownloadHead) => heads.push(head),
    bytes: () => Buffer.concat(chunks),
  };
}

test("parseContentRange reads the offsets and the length, and nothing else", () => {
  assert.deepEqual(parseContentRange("bytes 0-1023/4096"), { first: 0, last: 1023, length: 4096 });
  assert.equal(parseContentRange("bytes */4096"), undefined);
  assert.equal(parseContentRange("bytes 10-5/4096"), undefined);
  assert.equal(parseContentRange("bytes 0-4096/4096"), undefined);
  assert.equal(parseContentRange(null), undefined);
});

test("the storage errors are the API's codes, not a lock or a lack of access", () => {
  const read = (status: number, body: unknown, headers: Record<string, string> = {}) => {
    const response = jsonResponse(status, body, headers);
    return classify(response, JSON.stringify(body));
  };
  assert.deepEqual(read(403, { error: "sensitive" }), { ok: false, failure: { kind: "api", status: 403, code: "sensitive" } });
  assert.deepEqual(read(409, { error: "unavailable" }), { ok: false, failure: { kind: "api", status: 409, code: "unavailable" } });
  assert.deepEqual(read(416, { error: "range_not_satisfiable" }), {
    ok: false,
    failure: { kind: "api", status: 416, code: "range_not_satisfiable" },
  });
  assert.deepEqual(read(503, { error: "busy" }, { "Retry-After": "7" }), {
    ok: false,
    failure: { kind: "busy", retryAfterS: 7, code: "busy" },
  });
  assert.deepEqual(read(403, {}), { ok: false, failure: { kind: "forbidden" } });
  assert.deepEqual(read(409, {}), { ok: false, failure: { kind: "locked" } });
});

test("the typed client fills the path's parameters and passes the range headers", async () => {
  const { fake, api } = setup();
  const bytes = bytesOf(4_096);
  fake.reply(serveContent(bytes, { etag: '"a-0"' }));
  const sink = collect();
  const call = api.download("GetRegionContent", {
    params: { region: "app_current" },
    headers: { Range: "bytes=1024-2047", "If-Range": '"a-0"' },
    ...sink,
  });
  const outcome = await call.done;
  assert.equal(fake.calls[0]?.url, "/api/v1/storage/internal/regions/app_current/content");
  const headers = fake.calls[0]?.init.headers as Record<string, string>;
  assert.equal(headers["Range"], "bytes=1024-2047");
  assert.equal(headers["If-Range"], '"a-0"');
  assert.equal(outcome.ok, true);
  assert.deepEqual(sink.heads, [{ status: 206, etag: '"a-0"', length: 1_024, range: { first: 1_024, last: 2_047, length: 4_096 } }]);
  assert.deepEqual(sink.bytes(), Buffer.from(bytes.subarray(1_024, 2_048)));
});

test("fillPath encodes each parameter as one segment", () => {
  assert.equal(fillPath("/a/{region}/b", { region: "x/y" }), "/a/x%2Fy/b");
  assert.equal(fillPath("/a/{region}", undefined), "/a/{region}");
});

test("a download streams its body with progress, and the queue keeps flowing beside it", async () => {
  const { fake, transport } = setup();
  const bytes = bytesOf(8_192);
  const content = FakeFetch.deferred(new Response(bytes, { status: 200, headers: { "Content-Length": "8192", ETag: '"b-1"' } }));
  fake.reply(content.step, () => jsonResponse(200, { polled: true }));
  const sink = collect();
  const download = transport.download({ path: "/content", ...sink });
  await settle();
  assert.equal(transport.downloading, true);
  const poll = await transport.send({ method: "GET", path: "/system", idempotent: true, priority: "poll" });
  assert.equal(poll.ok, true);
  content.release();
  const outcome = await download.done;
  assert.equal(outcome.ok, true);
  assert.equal(outcome.received, 8_192);
  assert.deepEqual(sink.bytes(), Buffer.from(bytes));
  assert.equal(transport.downloading, false);
});

test("a download has no total timeout, only one for a stall", async () => {
  const { clock, fake, transport } = setup();
  const bytes = bytesOf(4_096);
  fake.reply(serveContent(bytes, { etag: '"c-1"', stallAfter: 2_048 }));
  const sink = collect();
  const download = transport.download({ path: "/content", ...sink });
  await clock.advance(STALL_MS - 1);
  assert.equal(transport.downloading, true);
  await clock.advance(1);
  const outcome = await download.done;
  assert.equal(outcome.ok, false);
  assert.deepEqual(!outcome.ok && outcome.failure, { kind: "timeout", afterMs: STALL_MS });
  assert.equal(outcome.received, 2_048);
  assert.equal(outcome.ok === false && outcome.head?.etag, '"c-1"');
});

test("a device that never answers stalls the download too", async () => {
  const { clock, fake, transport } = setup();
  fake.reply(FakeFetch.hang);
  const download = transport.download({ path: "/content", onChunk: () => {} });
  await clock.advance(STALL_MS);
  const outcome = await download.done;
  assert.deepEqual(!outcome.ok && outcome.failure, { kind: "timeout", afterMs: STALL_MS });
});

test("a cut body is a network failure that keeps the bytes that came, and isn't retried", async () => {
  const { fake, transport } = setup();
  fake.reply(serveContent(bytesOf(4_096), { cutAfter: 3_000 }));
  const sink = collect();
  const outcome = await transport.download({ path: "/content", ...sink }).done;
  assert.deepEqual(!outcome.ok && outcome.failure, { kind: "network" });
  assert.equal(outcome.received, 3_000);
  assert.equal(sink.bytes().byteLength, 3_000);
  assert.equal(fake.calls.length, 1);
});

test("a body shorter than its Content-Length is a network failure", async () => {
  const { fake, transport } = setup();
  fake.reply(() => new Response(bytesOf(100), { status: 200, headers: { "Content-Length": "200" } }));
  const outcome = await transport.download({ path: "/content", onChunk: () => {} }).done;
  assert.deepEqual(!outcome.ok && outcome.failure, { kind: "network" });
});

test("cancelling aborts the download", async () => {
  const { clock, fake, transport, attempts } = setup();
  fake.reply(serveContent(bytesOf(4_096), { stallAfter: 1_024 }));
  const download = transport.download({ path: "/content", onChunk: () => {} });
  await clock.advance(10);
  download.cancel();
  const outcome = await download.done;
  assert.deepEqual(!outcome.ok && outcome.failure, { kind: "aborted" });
  assert.deepEqual(attempts, [true]);
});

test("a busy device's answer comes back with its Retry-After, and the transport doesn't retry it", async () => {
  const { clock, fake, transport } = setup();
  fake.reply(() => jsonResponse(503, { error: "busy" }, { "Retry-After": "4" }));
  const download = transport.download({ path: "/content", onChunk: () => {} });
  await clock.advance(60_000);
  const outcome = await download.done;
  assert.deepEqual(!outcome.ok && outcome.failure, { kind: "busy", retryAfterS: 4, code: "busy" });
  assert.equal(fake.calls.length, 1);
});

test("only one download runs at a time", async () => {
  const { clock, fake, transport } = setup();
  fake.reply(serveContent(bytesOf(4_096), { stallAfter: 0 }));
  const first = transport.download({ path: "/one", onChunk: () => {} });
  const second = await transport.download({ path: "/two", onChunk: () => {} }).done;
  assert.equal(!second.ok && second.failure.kind, "busy");
  first.cancel();
  await first.done;
  await clock.advance(1);
  assert.equal(fake.calls.length, 1);
});
