import type { Result } from "../api/client.ts";
import type { JsonValue } from "../lib/json.ts";
import type { ConfigPath } from "./diff.ts";

/** Names a version of the running configuration. Only the device interprets it. */
export type Revision = string;

/** The running configuration and which version of it that is. */
export interface Snapshot {
  revision: Revision;
  config: JsonValue;
}

/** An exclusive hold on editing, which lapses unless it is renewed. */
export interface Lock {
  id: string;
  /** Milliseconds since the epoch. */
  expiresAt: number;
}

export interface LockStatus {
  /** Who holds the lock; `undefined` if nobody does. */
  holder?: string;
  /** Milliseconds since the epoch. */
  since?: number;
}

export interface Problem {
  path: ConfigPath;
  message: string;
}

/** What the device makes of a candidate without applying it. */
export interface CheckReport {
  problems: readonly Problem[];
}

export interface CommitOptions {
  /**
   * Roll the change back unless it is confirmed within this many seconds, as
   * a way to try a change that could cut the person off from the device.
   */
  confirmWithinS?: number;
}

export interface CommitOutcome {
  /** The revision the commit made the running configuration. */
  revision: Revision;
  /** When an unconfirmed commit rolls back, in milliseconds since the epoch. */
  confirmDeadline?: number;
}

/**
 * What the device offers for editing its persistent configuration, in the
 * manner of JunOS: an exclusive lock, a candidate checked before it is
 * committed, and commits that roll themselves back unless confirmed.
 */
export interface ConfigService {
  running(): Promise<Result<Snapshot>>;
  lock(): Promise<Result<Lock>>;
  renew(lock: Lock): Promise<Result<Lock>>;
  unlock(lock: Lock): Promise<Result<undefined>>;
  check(candidate: JsonValue): Promise<Result<CheckReport>>;
  /** Commits `candidate` as one atomic change, if the running configuration is still at `baseRevision`. */
  commit(
    lock: Lock,
    baseRevision: Revision,
    candidate: JsonValue,
    options: CommitOptions,
  ): Promise<Result<CommitOutcome>>;
  confirm(lock: Lock): Promise<Result<undefined>>;
  status(): Promise<Result<LockStatus>>;
}
