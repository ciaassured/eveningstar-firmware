import assert from "node:assert/strict";
import { test } from "node:test";
import type { Result } from "../api/client.ts";
import type { ApiFailure, FailureContext } from "../api/failure.ts";
import type { JsonValue } from "../lib/json.ts";
import { FakeClock, settle } from "../testing/fakes.ts";
import { diffConfig, formatChanges } from "./diff.ts";
import { createEditController, RENEW_RETRY_MS } from "./edit-controller.ts";
import { canCommit, canRelock, hasUnsavedEdits, reconcile, reduceEdit } from "./edit-session.ts";
import type { EditEvent, EditState } from "./edit-session.ts";
import type { CheckReport, CommitOutcome, ConfigService, Lock, LockStatus, Snapshot } from "./service.ts";

const context: FailureContext = { method: "POST", path: "/x", attempts: 1, at: 0 };
const ok = <T>(value: T): Result<T> => ({ ok: true, value, context });
const fail = <T>(failure: ApiFailure): Result<T> => ({ ok: false, failure, context });

const running: Snapshot = { revision: "r1", config: { name: "a", ntp: ["x"], wifi: { ssid: "home" } } };
const lock = (expiresAt = 60_000): Lock => ({ id: "L1", expiresAt });

function run(events: EditEvent[], from: EditState = { kind: "operational" }): EditState {
  return events.reduce(reduceEdit, from);
}

const editing = (): EditState =>
  run([{ type: "edit-requested" }, { type: "lock-acquired", lock: lock(), running }]);

test("diffConfig lists added, removed and changed values by path, and never holds a secret", () => {
  const candidate: JsonValue = { name: "b", ntp: ["x", "y"], wifi: { ssid: "home", psk: "hunter2" }, extra: 1 };
  const changes = diffConfig(running.config, candidate, { isSecret: (path) => path.at(-1) === "psk" });
  assert.deepEqual(changes, [
    { kind: "added", path: ["extra"], after: 1 },
    { kind: "changed", path: ["name"], before: "a", after: "b" },
    { kind: "added", path: ["ntp", 1], after: "y" },
    { kind: "secret", path: ["wifi", "psk"] },
  ]);
  assert.ok(!JSON.stringify(changes).includes("hunter2"));
  assert.deepEqual(formatChanges(changes), [
    "+ extra: 1",
    '- name: "a"',
    '+ name: "b"',
    '+ ntp.1: "y"',
    "~ wifi.psk: secret changed",
  ]);
});

test("diffConfig finds nothing between equal values, however their keys are ordered", () => {
  assert.deepEqual(diffConfig({ a: 1, b: [1, 2] }, { b: [1, 2], a: 1 }), []);
  assert.deepEqual(diffConfig(running.config, running.config), []);
});

test("diffConfig reports removed keys and shortened arrays", () => {
  const changes = diffConfig({ a: 1, list: [1, 2] }, { list: [1] });
  assert.deepEqual(changes, [
    { kind: "removed", path: ["a"], before: 1 },
    { kind: "removed", path: ["list", 1], before: 2 },
  ]);
});

test("editing starts from the running configuration and is clean until the candidate changes", () => {
  const state = editing();
  assert.equal(state.kind, "editing");
  assert.equal(hasUnsavedEdits(state), false);
  assert.equal(canCommit(state), false);
  const edited = reduceEdit(state, { type: "candidate-changed", candidate: { ...running.config as object, name: "b" } });
  assert.equal(hasUnsavedEdits(edited), true);
  assert.equal(canCommit(edited), true);
});

test("another person's lock is reported with who holds it, and editing can be asked for again", () => {
  const state = run([{ type: "edit-requested" }, { type: "lock-denied", holder: "operator" }]);
  assert.deepEqual(state, { kind: "locked-by-other", holder: "operator" });
  assert.equal(reduceEdit(state, { type: "edit-requested" }).kind, "acquiring");
});

test("a failed attempt to lock is kept with its failure", () => {
  const state = run([{ type: "edit-requested" }, { type: "lock-failed", failure: { kind: "network" } }]);
  assert.deepEqual(state, { kind: "failed", failure: { kind: "network" } });
});

test("a check leaves the report on the editing state until the candidate changes again", () => {
  const report: CheckReport = { problems: [{ path: ["name"], message: "too long" }] };
  let state = run([{ type: "check-started" }, { type: "check-finished", report }], editing());
  assert.equal(state.kind === "editing" && state.report, report);
  state = reduceEdit(state, { type: "candidate-changed", candidate: { name: "c" } });
  assert.equal(state.kind === "editing" && state.report, undefined);
});

test("a commit that needs confirming waits for it, then returns to operational", () => {
  const edited = reduceEdit(editing(), { type: "candidate-changed", candidate: { name: "b" } });
  let state = run([{ type: "commit-started", confirmWithinS: 60 }, { type: "commit-applied", revision: "r2", deadline: 90_000 }], edited);
  assert.deepEqual(state, { kind: "awaiting-confirm", lock: lock(), revision: "r2", deadline: 90_000 });
  assert.equal(reduceEdit(state, { type: "confirmed" }).kind, "operational");
  state = reduceEdit(state, { type: "rolled-back" });
  assert.equal(state.kind, "operational");
});

test("a commit without confirmation ends editing", () => {
  const edited = reduceEdit(editing(), { type: "candidate-changed", candidate: { name: "b" } });
  assert.equal(run([{ type: "commit-started" }, { type: "commit-applied", revision: "r2" }], edited).kind, "operational");
});

test("a refused commit returns to editing with the candidate", () => {
  const edited = reduceEdit(editing(), { type: "candidate-changed", candidate: { name: "b" } });
  const state = run([{ type: "commit-started" }, { type: "commit-refused" }], edited);
  assert.equal(state.kind, "editing");
  assert.deepEqual(state.kind === "editing" && state.candidate, { name: "b" });
});

test("losing the lock keeps the candidate, and it can be taken again only if the running revision hasn't moved", () => {
  const edited = reduceEdit(editing(), { type: "candidate-changed", candidate: { name: "b" } });
  const lost = reduceEdit(edited, { type: "lock-lost" });
  assert.equal(lost.kind, "lock-lost");
  assert.equal(hasUnsavedEdits(lost), true);
  assert.equal(canCommit(lost), false);
  assert.equal(canRelock(lost, running), true);
  assert.equal(canRelock(lost, { revision: "r9", config: {} }), false);
  const back = reduceEdit(lost, { type: "relocked", lock: lock(120_000), running });
  assert.equal(back.kind, "editing");
  assert.deepEqual(back.kind === "editing" && back.candidate, { name: "b" });
});

test("a lost commit answer is an unknown outcome, and the running configuration tells which", () => {
  const candidate: JsonValue = { name: "b" };
  const unknown = run(
    [{ type: "candidate-changed", candidate }, { type: "commit-started" }, { type: "commit-unknown" }],
    editing(),
  );
  assert.equal(unknown.kind, "unknown-outcome");
  assert.equal(reconcile(unknown, running), "not-applied");
  assert.equal(reconcile(unknown, { revision: "r2", config: candidate }), "applied");
  assert.equal(reconcile(unknown, { revision: "r2", config: { name: "other" } }), "diverged");
});

test("events that don't fit the state change nothing", () => {
  const operational: EditState = { kind: "operational" };
  for (const event of [
    { type: "lock-acquired", lock: lock(), running },
    { type: "check-started" },
    { type: "commit-started" },
    { type: "confirmed" },
    { type: "lock-lost" },
  ] satisfies EditEvent[]) {
    assert.deepEqual(reduceEdit(operational, event), operational);
  }
  assert.equal(reduceEdit(editing(), { type: "left" }).kind, "operational");
});

class FakeService implements ConfigService {
  calls: string[] = [];
  lockAnswer: Result<Lock> = ok(lock());
  runningAnswers: Result<Snapshot>[] = [ok(running)];
  renewAnswers: Result<Lock>[] = [];
  checkAnswer: Result<CheckReport> = ok({ problems: [] });
  commitAnswer: Result<CommitOutcome> = ok({ revision: "r2" });
  confirmAnswer: Result<undefined> = ok(undefined);

  running(): Promise<Result<Snapshot>> {
    this.calls.push("running");
    const answer = this.runningAnswers.length > 1 ? this.runningAnswers.shift() : this.runningAnswers[0];
    return Promise.resolve(answer ?? fail<Snapshot>({ kind: "network" }));
  }
  lock() {
    this.calls.push("lock");
    return Promise.resolve(this.lockAnswer);
  }
  renew(held: Lock) {
    this.calls.push("renew");
    return Promise.resolve(this.renewAnswers.shift() ?? ok({ ...held, expiresAt: held.expiresAt + 60_000 }));
  }
  unlock() {
    this.calls.push("unlock");
    return Promise.resolve(ok(undefined));
  }
  check() {
    this.calls.push("check");
    return Promise.resolve(this.checkAnswer);
  }
  commit() {
    this.calls.push("commit");
    return Promise.resolve(this.commitAnswer);
  }
  confirm() {
    this.calls.push("confirm");
    return Promise.resolve(this.confirmAnswer);
  }
  status() {
    this.calls.push("status");
    return Promise.resolve(ok<LockStatus>({}));
  }
}

function controller() {
  const clock = new FakeClock();
  const service = new FakeService();
  const edit = createEditController({ service, timers: clock, now: () => clock.now });
  return { clock, service, edit };
}

test("begin takes the lock, reads the running configuration and starts editing", async () => {
  const { service, edit } = controller();
  await edit.begin();
  assert.deepEqual(service.calls, ["lock", "running"]);
  assert.equal(edit.state.value.kind, "editing");
  edit.dispose();
});

test("begin reports who holds the lock when it is taken", async () => {
  const { service, edit } = controller();
  service.lockAnswer = fail({ kind: "locked", holder: "operator" });
  await edit.begin();
  assert.deepEqual(edit.state.value, { kind: "locked-by-other", holder: "operator" });
  assert.deepEqual(service.calls, ["lock"]);
});

test("begin lets go of the lock when the running configuration can't be read", async () => {
  const { service, edit } = controller();
  service.runningAnswers = [fail({ kind: "timeout", afterMs: 8_000 })];
  await edit.begin();
  assert.deepEqual(service.calls, ["lock", "running", "unlock"]);
  assert.equal(edit.state.value.kind, "failed");
});

test("the lease is renewed halfway through, and lapses if it can't be", async () => {
  const { clock, service, edit } = controller();
  service.lockAnswer = ok(lock(10_000));
  await edit.begin();
  await clock.advance(5_000);
  assert.equal(service.calls.filter((call) => call === "renew").length, 1);
  assert.equal(edit.state.value.kind === "editing" && edit.state.value.lock.expiresAt, 70_000);

  // The next renewal is due halfway to the new expiry, at 37.5 s.
  service.renewAnswers = Array.from({ length: 20 }, () => fail<Lock>({ kind: "network" }));
  await clock.advance(32_500);
  let state = edit.state.value;
  assert.equal(state.kind === "editing" && state.renewFailed, true);
  await clock.advance(RENEW_RETRY_MS);
  assert.equal(service.calls.filter((call) => call === "renew").length, 3);
  state = edit.state.value;
  assert.equal(state.kind === "editing" && state.renewFailed, true);
  await clock.advance(70_000 - clock.now);
  assert.equal(edit.state.value.kind, "lock-lost");
  edit.dispose();
});

test("a lease the device no longer recognises is lost, and the candidate stays", async () => {
  const { clock, service, edit } = controller();
  service.lockAnswer = ok(lock(10_000));
  await edit.begin();
  edit.edit({ name: "mine" });
  service.renewAnswers = [fail({ kind: "api", status: 409, code: "gone" })];
  await clock.advance(5_000);
  const state = edit.state.value;
  assert.equal(state.kind, "lock-lost");
  assert.deepEqual(state.kind === "lock-lost" && state.candidate, { name: "mine" });
  edit.dispose();
});

test("a lease that runs out is lost", async () => {
  const { clock, service, edit } = controller();
  service.lockAnswer = ok(lock(10_000));
  service.renewAnswers = [fail({ kind: "network" })];
  await edit.begin();
  await clock.advance(10_000);
  assert.equal(edit.state.value.kind, "lock-lost");
  edit.dispose();
});

test("check keeps the report, and a commit sends the lock, base revision and candidate", async () => {
  const { service, edit } = controller();
  await edit.begin();
  edit.edit({ name: "b" });
  await edit.check();
  assert.equal(edit.state.value.kind, "editing");
  await edit.commit();
  assert.deepEqual(service.calls, ["lock", "running", "check", "commit", "unlock"]);
  assert.equal(edit.state.value.kind, "operational");
  edit.dispose();
});

test("a commit to be confirmed keeps the lock, and rolls back when the time runs out", async () => {
  const { clock, service, edit } = controller();
  service.commitAnswer = ok({ revision: "r2", confirmDeadline: 90_000 });
  await edit.begin();
  edit.edit({ name: "b" });
  await edit.commit({ confirmWithinS: 60 });
  assert.equal(edit.state.value.kind, "awaiting-confirm");
  assert.ok(!service.calls.includes("unlock"));
  await clock.advance(89_999);
  assert.equal(edit.state.value.kind, "awaiting-confirm");
  await clock.advance(1);
  assert.equal(edit.state.value.kind, "operational");
  edit.dispose();
});

test("confirming a commit ends the wait and lets go of the lock", async () => {
  const { service, edit } = controller();
  service.commitAnswer = ok({ revision: "r2", confirmDeadline: 90_000 });
  await edit.begin();
  edit.edit({ name: "b" });
  await edit.commit({ confirmWithinS: 60 });
  await edit.confirm();
  assert.equal(edit.state.value.kind, "operational");
  assert.deepEqual(service.calls.slice(-2), ["confirm", "unlock"]);
  edit.dispose();
});

test("a commit whose answer is lost is reconciled by reading the running configuration", async () => {
  const { service, edit } = controller();
  await edit.begin();
  edit.edit({ name: "b" });
  service.commitAnswer = fail({ kind: "unknown-outcome", cause: { kind: "network" } });
  await edit.commit();
  assert.equal(edit.state.value.kind, "unknown-outcome");
  service.runningAnswers = [ok({ revision: "r2", config: { name: "b" } })];
  assert.equal(await edit.resolve(), "applied");
  assert.equal(edit.state.value.kind, "operational");
  edit.dispose();
});

test("a commit that wasn't applied can be locked again and tried again", async () => {
  const { service, edit } = controller();
  await edit.begin();
  edit.edit({ name: "b" });
  service.commitAnswer = fail({ kind: "unknown-outcome", cause: { kind: "timeout", afterMs: 10_000 } });
  await edit.commit();
  service.runningAnswers = [ok(running)];
  assert.equal(await edit.resolve(), "not-applied");
  assert.equal(await edit.relock(), true);
  assert.equal(edit.state.value.kind, "editing");
  edit.dispose();
});

test("leaving lets go of the lock", async () => {
  const { service, edit } = controller();
  await edit.begin();
  await edit.leave();
  assert.equal(edit.state.value.kind, "operational");
  assert.equal(service.calls.at(-1), "unlock");
  await settle();
  edit.dispose();
});
