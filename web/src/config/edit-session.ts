import type { ApiFailure } from "../api/failure.ts";
import type { JsonValue } from "../lib/json.ts";
import { jsonEqual } from "../lib/json.ts";
import type { CheckReport, Lock, Revision, Snapshot } from "./service.ts";

/** The candidate being edited, and what it started from. */
interface Draft {
  base: Snapshot;
  candidate: JsonValue;
}

/**
 * Where editing the configuration has got to. Outside it the page is
 * operational: it reads and acts, but changes nothing persistent.
 */
export type EditState =
  | { kind: "operational" }
  | { kind: "acquiring" }
  | { kind: "locked-by-other"; holder?: string; since?: number }
  | { kind: "failed"; failure: ApiFailure }
  | ({ kind: "editing"; lock: Lock; renewFailed: boolean; report?: CheckReport } & Draft)
  | ({ kind: "checking"; lock: Lock } & Draft)
  | ({ kind: "committing"; lock: Lock; confirmWithinS?: number } & Draft)
  | { kind: "awaiting-confirm"; lock: Lock; revision: Revision; deadline: number }
  /** The lock lapsed. The candidate is kept, but can't be committed unless the person locks again. */
  | ({ kind: "lock-lost" } & Draft)
  /** The commit's answer was lost; whether it applied is not known. */
  | ({ kind: "unknown-outcome" } & Draft);

export type EditEvent =
  | { type: "edit-requested" }
  | { type: "lock-acquired"; lock: Lock; running: Snapshot }
  | { type: "lock-denied"; holder?: string; since?: number }
  | { type: "lock-failed"; failure: ApiFailure }
  | { type: "candidate-changed"; candidate: JsonValue }
  | { type: "lease-renewed"; lock: Lock }
  | { type: "lease-renewal-failed" }
  | { type: "lock-lost" }
  | { type: "check-started" }
  | { type: "check-finished"; report: CheckReport }
  | { type: "check-failed" }
  | { type: "commit-started"; confirmWithinS?: number }
  | { type: "commit-applied"; revision: Revision; deadline?: number }
  | { type: "commit-refused" }
  | { type: "commit-unknown" }
  | { type: "relocked"; lock: Lock; running: Snapshot }
  | { type: "confirmed" }
  | { type: "rolled-back" }
  | { type: "left" };

function draft(state: EditState): Draft | undefined {
  switch (state.kind) {
    case "editing":
    case "checking":
    case "committing":
    case "lock-lost":
    case "unknown-outcome":
      return { base: state.base, candidate: state.candidate };
    case "operational":
    case "acquiring":
    case "locked-by-other":
    case "failed":
    case "awaiting-confirm":
      return undefined;
  }
}

/** The reducer of the edit session: pure, so every path through it can be tested. */
export function reduceEdit(state: EditState, event: EditEvent): EditState {
  switch (event.type) {
    case "edit-requested":
      return state.kind === "operational" || state.kind === "locked-by-other" || state.kind === "failed"
        ? { kind: "acquiring" }
        : state;
    case "lock-acquired":
      return state.kind === "acquiring"
        ? {
            kind: "editing",
            lock: event.lock,
            renewFailed: false,
            base: event.running,
            candidate: event.running.config,
          }
        : state;
    case "lock-denied": {
      if (state.kind !== "acquiring") return state;
      const other: EditState = { kind: "locked-by-other" };
      if (event.holder !== undefined) other.holder = event.holder;
      if (event.since !== undefined) other.since = event.since;
      return other;
    }
    case "lock-failed":
      return state.kind === "acquiring" ? { kind: "failed", failure: event.failure } : state;
    case "candidate-changed": {
      if (state.kind === "editing") {
        const { report: _stale, ...rest } = state;
        return { ...rest, candidate: event.candidate };
      }
      return state.kind === "lock-lost" ? { ...state, candidate: event.candidate } : state;
    }
    case "lease-renewed":
      return state.kind === "editing" || state.kind === "checking" || state.kind === "committing"
        ? { ...state, lock: event.lock, ...(state.kind === "editing" ? { renewFailed: false } : {}) }
        : state;
    case "lease-renewal-failed":
      return state.kind === "editing" ? { ...state, renewFailed: true } : state;
    case "lock-lost": {
      const kept = draft(state);
      return kept !== undefined && state.kind !== "unknown-outcome" && state.kind !== "lock-lost"
        ? { kind: "lock-lost", ...kept }
        : state;
    }
    case "check-started":
      return state.kind === "editing"
        ? { kind: "checking", lock: state.lock, base: state.base, candidate: state.candidate }
        : state;
    case "check-finished":
      return state.kind === "checking"
        ? {
            kind: "editing",
            lock: state.lock,
            renewFailed: false,
            base: state.base,
            candidate: state.candidate,
            report: event.report,
          }
        : state;
    case "check-failed":
      return state.kind === "checking"
        ? { kind: "editing", lock: state.lock, renewFailed: false, base: state.base, candidate: state.candidate }
        : state;
    case "commit-started":
      return state.kind === "editing"
        ? {
            kind: "committing",
            lock: state.lock,
            base: state.base,
            candidate: state.candidate,
            ...(event.confirmWithinS === undefined ? {} : { confirmWithinS: event.confirmWithinS }),
          }
        : state;
    case "commit-applied": {
      if (state.kind !== "committing") return state;
      return event.deadline === undefined
        ? { kind: "operational" }
        : { kind: "awaiting-confirm", lock: state.lock, revision: event.revision, deadline: event.deadline };
    }
    case "commit-refused":
      return state.kind === "committing"
        ? { kind: "editing", lock: state.lock, renewFailed: false, base: state.base, candidate: state.candidate }
        : state;
    case "commit-unknown":
      return state.kind === "committing"
        ? { kind: "unknown-outcome", base: state.base, candidate: state.candidate }
        : state;
    case "relocked":
      return state.kind === "lock-lost" || state.kind === "unknown-outcome"
        ? {
            kind: "editing",
            lock: event.lock,
            renewFailed: false,
            base: event.running,
            candidate: state.candidate,
          }
        : state;
    case "confirmed":
    case "rolled-back":
      return state.kind === "awaiting-confirm" ? { kind: "operational" } : state;
    case "left":
      return { kind: "operational" };
  }
}

/** Whether leaving would lose edits: the candidate differs from what it started from, or hasn't been committed. */
export function hasUnsavedEdits(state: EditState): boolean {
  const kept = draft(state);
  return kept !== undefined && !jsonEqual(kept.base.config, kept.candidate);
}

/** Whether the candidate can be committed: it was edited, and the lock is held. */
export function canCommit(state: EditState): boolean {
  return state.kind === "editing" && !jsonEqual(state.base.config, state.candidate);
}

/** Whether a lost lock can be taken again with the candidate: the running configuration is where the candidate started. */
export function canRelock(state: EditState, running: Snapshot): boolean {
  return (
    (state.kind === "lock-lost" || state.kind === "unknown-outcome") && state.base.revision === running.revision
  );
}

/** What the running configuration says about a commit whose answer was lost. */
export type Reconciliation = "applied" | "not-applied" | "diverged";

export function reconcile(state: EditState, running: Snapshot): Reconciliation {
  if (state.kind !== "unknown-outcome") return "not-applied";
  if (running.revision === state.base.revision) return "not-applied";
  return jsonEqual(running.config, state.candidate) ? "applied" : "diverged";
}
