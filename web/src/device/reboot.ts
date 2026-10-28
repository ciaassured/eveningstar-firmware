import type { ReadonlySignal } from "@preact/signals";
import { signal } from "@preact/signals";
import type { Result } from "../api/client.ts";
import type { ApiFailure, FailureContext } from "../api/failure.ts";
import type { RebootMode, RebootStatus, System } from "./types.ts";

/** Where a reboot the person asked for has got to. */
export type RebootProgress =
  | { kind: "idle" }
  | { kind: "requesting" }
  /** The request is in, or its outcome is unknown; the device is still answering. */
  | { kind: "waiting"; status?: RebootStatus; fetchedAt?: number }
  | { kind: "restarting" }
  | { kind: "restarted" }
  | { kind: "timed-out" }
  | { kind: "failed"; failure: ApiFailure; context: FailureContext };

/** The calls the flow makes, as the device store provides them. */
export interface RebootApi {
  reboot(mode: RebootMode): Promise<Result<undefined>>;
  status(): Promise<Result<RebootStatus>>;
  system(): Promise<Result<System>>;
}

export interface RebootDeps {
  api: RebootApi;
  sleep(ms: number): Promise<void>;
  /** Milliseconds since the epoch. */
  now: () => number;
}

/** How often the flow asks the device how it is getting on. */
export const POLL_INTERVAL_MS = 2_000;
/** How long the device has to come back. */
export const RESTART_TIMEOUT_MS = 120_000;

export interface RebootFlow {
  readonly progress: ReadonlySignal<RebootProgress>;
  /**
   * Asks the device to reboot and follows it down and up again. `uptimeBefore`
   * is the uptime the page last saw: an uptime lower than that means the
   * device has restarted. Settles when the flow ends.
   */
  start(mode: RebootMode, uptimeBefore: number): Promise<void>;
  /** Returns to idle, such as when the person closes the result. */
  reset(): void;
}

export function createRebootFlow(deps: RebootDeps): RebootFlow {
  const progress = signal<RebootProgress>({ kind: "idle" });
  let running = false;

  async function follow(uptimeBefore: number): Promise<void> {
    const deadline = deps.now() + RESTART_TIMEOUT_MS;
    let restarting = false;
    let status: RebootStatus | undefined;

    while (deps.now() < deadline) {
      const reboot = await deps.api.status();
      if (deps.now() >= deadline) break;
      if (reboot.ok) {
        status = reboot.value;
        if (!restarting) progress.value = { kind: "waiting", status, fetchedAt: deps.now() };
      } else {
        restarting = true;
        progress.value = { kind: "restarting" };
      }

      const system = await deps.api.system();
      if (deps.now() >= deadline && !(system.ok && system.value.uptime_s < uptimeBefore)) break;
      if (system.ok) {
        if (system.value.uptime_s < uptimeBefore) {
          progress.value = { kind: "restarted" };
          return;
        }
      } else {
        restarting = true;
        progress.value = { kind: "restarting" };
      }
      await deps.sleep(POLL_INTERVAL_MS);
    }
    progress.value = { kind: "timed-out" };
  }

  return {
    progress,
    async start(mode, uptimeBefore) {
      if (running) return;
      running = true;
      try {
        progress.value = { kind: "requesting" };
        const request = await deps.api.reboot(mode);
        // A refusal is known; losing the connection may mean the reboot began.
        if (!request.ok && request.failure.kind !== "unknown-outcome") {
          progress.value = { kind: "failed", failure: request.failure, context: request.context };
          return;
        }
        progress.value = { kind: "waiting" };
        await follow(uptimeBefore);
      } finally {
        running = false;
      }
    },
    reset() {
      if (!running) progress.value = { kind: "idle" };
    },
  };
}
