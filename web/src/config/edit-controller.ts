import type { ReadonlySignal } from "@preact/signals";
import { signal } from "@preact/signals";
import type { ApiFailure } from "../api/failure.ts";
import type { Timers } from "../api/transport.ts";
import type { JsonValue } from "../lib/json.ts";
import { reconcile, reduceEdit } from "./edit-session.ts";
import type { EditEvent, EditState, Reconciliation } from "./edit-session.ts";
import type { CommitOptions, ConfigService, Lock, Snapshot } from "./service.ts";

/** How long to wait before renewing a lease again after a renewal failed. */
export const RENEW_RETRY_MS = 5_000;

export interface EditControllerDeps {
  service: ConfigService;
  timers: Timers;
  /** Milliseconds since the epoch. */
  now: () => number;
}

/**
 * Runs the effects of the edit session: taking the lock, keeping it while the
 * person edits, checking and committing, and following a commit that must be
 * confirmed. Its state is the reducer's.
 */
export interface EditController {
  readonly state: ReadonlySignal<EditState>;
  /** Why the latest request of the device failed, if it did. */
  readonly failure: ReadonlySignal<ApiFailure | undefined>;
  begin(): Promise<void>;
  edit(candidate: JsonValue): void;
  check(): Promise<void>;
  commit(options?: CommitOptions): Promise<void>;
  confirm(): Promise<void>;
  /** Takes the lock again, with the candidate kept, if the running configuration hasn't moved. */
  relock(): Promise<boolean>;
  /** Reads the running configuration to learn what became of a commit whose answer was lost. */
  resolve(): Promise<Reconciliation | undefined>;
  /** Leaves editing, letting go of the lock. */
  leave(): Promise<void>;
  dispose(): void;
}

function lockOf(state: EditState): Lock | undefined {
  switch (state.kind) {
    case "editing":
    case "checking":
    case "committing":
    case "awaiting-confirm":
      return state.lock;
    case "operational":
    case "acquiring":
    case "locked-by-other":
    case "failed":
    case "lock-lost":
    case "unknown-outcome":
      return undefined;
  }
}

export function createEditController(deps: EditControllerDeps): EditController {
  const { service, timers } = deps;
  const state = signal<EditState>({ kind: "operational" });
  const failure = signal<ApiFailure | undefined>(undefined);
  let renewal: unknown;
  let expiry: unknown;
  let deadline: unknown;
  let watching: Lock | undefined;
  let watchedDeadline: number | undefined;

  function dispatch(event: EditEvent): void {
    state.value = reduceEdit(state.value, event);
  }

  function clearTimers(): void {
    timers.clearTimeout(renewal);
    timers.clearTimeout(expiry);
  }

  function renew(lock: Lock): void {
    void service.renew(lock).then((result) => {
      if (lockOf(state.peek())?.id !== lock.id) return;
      if (result.ok) {
        dispatch({ type: "lease-renewed", lock: result.value });
        return;
      }
      const lost = ["locked", "forbidden", "api"].includes(result.failure.kind);
      if (lost) {
        dispatch({ type: "lock-lost" });
        return;
      }
      dispatch({ type: "lease-renewal-failed" });
      renewal = timers.setTimeout(() => {
        renew(lock);
      }, RENEW_RETRY_MS);
    });
  }

  // Keeps the lease while the lock is held, and notices when it lapses.
  state.subscribe((current) => {
    const lock = lockOf(current);
    if (current.kind === "awaiting-confirm") {
      if (watchedDeadline !== current.deadline) {
        watchedDeadline = current.deadline;
        timers.clearTimeout(deadline);
        deadline = timers.setTimeout(() => {
          dispatch({ type: "rolled-back" });
        }, Math.max(0, current.deadline - deps.now()));
      }
    } else {
      watchedDeadline = undefined;
      timers.clearTimeout(deadline);
    }
    if (lock === undefined || current.kind === "awaiting-confirm") {
      clearTimers();
      watching = undefined;
      return;
    }
    if (watching?.id === lock.id && watching.expiresAt === lock.expiresAt) return;
    clearTimers();
    watching = lock;
    const left = lock.expiresAt - deps.now();
    renewal = timers.setTimeout(() => {
      renew(lock);
    }, Math.max(0, left / 2));
    expiry = timers.setTimeout(() => {
      dispatch({ type: "lock-lost" });
    }, Math.max(0, left));
  });

  async function release(lock: Lock | undefined): Promise<void> {
    if (lock !== undefined) await service.unlock(lock);
  }

  async function takeLock(onTaken: (lock: Lock, running: Snapshot) => void): Promise<void> {
    const lock = await service.lock();
    if (!lock.ok) {
      if (lock.failure.kind === "locked") {
        const denied: EditEvent = { type: "lock-denied" };
        if (lock.failure.holder !== undefined) denied.holder = lock.failure.holder;
        dispatch(denied);
      } else {
        failure.value = lock.failure;
        dispatch({ type: "lock-failed", failure: lock.failure });
      }
      return;
    }
    const running = await service.running();
    if (!running.ok) {
      await release(lock.value);
      failure.value = running.failure;
      dispatch({ type: "lock-failed", failure: running.failure });
      return;
    }
    onTaken(lock.value, running.value);
  }

  return {
    state,
    failure,
    async begin() {
      if (!["operational", "locked-by-other", "failed"].includes(state.peek().kind)) return;
      failure.value = undefined;
      dispatch({ type: "edit-requested" });
      await takeLock((lock, running) => {
        dispatch({ type: "lock-acquired", lock, running });
      });
    },
    edit(candidate) {
      dispatch({ type: "candidate-changed", candidate });
    },
    async check() {
      const current = state.peek();
      if (current.kind !== "editing") return;
      dispatch({ type: "check-started" });
      const result = await service.check(current.candidate);
      if (result.ok) dispatch({ type: "check-finished", report: result.value });
      else {
        failure.value = result.failure;
        dispatch({ type: "check-failed" });
      }
    },
    async commit(options = {}) {
      const current = state.peek();
      if (current.kind !== "editing") return;
      failure.value = undefined;
      dispatch(
        options.confirmWithinS === undefined
          ? { type: "commit-started" }
          : { type: "commit-started", confirmWithinS: options.confirmWithinS },
      );
      const result = await service.commit(current.lock, current.base.revision, current.candidate, options);
      if (result.ok) {
        const applied: EditEvent = { type: "commit-applied", revision: result.value.revision };
        if (result.value.confirmDeadline !== undefined) applied.deadline = result.value.confirmDeadline;
        dispatch(applied);
        if (result.value.confirmDeadline === undefined) await release(current.lock);
        return;
      }
      failure.value = result.failure;
      dispatch({ type: result.failure.kind === "unknown-outcome" ? "commit-unknown" : "commit-refused" });
    },
    async confirm() {
      const current = state.peek();
      if (current.kind !== "awaiting-confirm") return;
      const result = await service.confirm(current.lock);
      if (result.ok) {
        dispatch({ type: "confirmed" });
        await release(current.lock);
      } else failure.value = result.failure;
    },
    async relock() {
      const current = state.peek();
      if (current.kind !== "lock-lost" && current.kind !== "unknown-outcome") return false;
      let taken = false;
      await takeLock((lock, running) => {
        if (running.revision !== current.base.revision) {
          void release(lock);
          return;
        }
        dispatch({ type: "relocked", lock, running });
        taken = true;
      });
      return taken;
    },
    async resolve() {
      const current = state.peek();
      if (current.kind !== "unknown-outcome") return undefined;
      const running = await service.running();
      if (!running.ok) {
        failure.value = running.failure;
        return undefined;
      }
      const outcome = reconcile(current, running.value);
      if (outcome === "applied") dispatch({ type: "left" });
      return outcome;
    },
    async leave() {
      const lock = lockOf(state.peek());
      dispatch({ type: "left" });
      await release(lock);
    },
    dispose() {
      clearTimers();
      timers.clearTimeout(deadline);
    },
  };
}
