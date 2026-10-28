import type { ReadonlySignal } from "@preact/signals";
import { signal } from "@preact/signals";
import type { ApiClient } from "../api/client.ts";
import { callOptions } from "../api/client.ts";
import type { Timers } from "../api/transport.ts";
import type { LiveSource } from "../live/poller.ts";
import { createPoller } from "../live/poller.ts";
import { createRebootFlow } from "./reboot.ts";
import type { RebootFlow } from "./reboot.ts";
import type { RebootStatus, System } from "./types.ts";

/** How often live values of the device are fetched. */
export const POLL_INTERVAL_MS = 5_000;
/** The interval fetches back off to while they fail. */
export const MAX_POLL_INTERVAL_MS = 30_000;

export interface DeviceDeps {
  api: ApiClient;
  timers: Timers;
  now: () => number;
  /** Whether the page is shown and in use. */
  active: ReadonlySignal<boolean>;
}

/** What the page knows of the device. */
export interface Device {
  readonly system: LiveSource<System>;
  readonly rebootStatus: LiveSource<RebootStatus>;
  readonly reboot: RebootFlow;
  /** How many restarts of the device the page has seen: the uptime went down. */
  readonly restarts: ReadonlySignal<number>;
  /** Forgets everything fetched, such as when another person signs in. */
  reset(): void;
  start(): void;
  stop(): void;
}

export function createDevice(deps: DeviceDeps): Device {
  const { api } = deps;
  const restarts = signal(0);
  let lastUptime: number | undefined;

  const poller = { ...deps, intervalMs: POLL_INTERVAL_MS, maxIntervalMs: MAX_POLL_INTERVAL_MS };

  const system = createPoller<System>({
    ...poller,
    async fetch(priority, group) {
      const result = await api.request("GetSystem", callOptions(priority, group));
      if (result.ok) {
        // An uptime that went down means the device started again.
        if (lastUptime !== undefined && result.value.uptime_s < lastUptime) {
          restarts.value += 1;
          void rebootStatus.load("poll");
        }
        lastUptime = result.value.uptime_s;
      }
      return result;
    },
  });

  const rebootStatus = createPoller<RebootStatus>({
    ...poller,
    fetch: (priority, group) => api.request("GetRebootStatus", callOptions(priority, group)),
  });

  const reboot = createRebootFlow({
    api: {
      reboot: (mode) => api.request("Reboot", { priority: "action", body: { mode } }),
      // The flow polls by itself, and a device that is down has to show as down at once.
      status: () => api.request("GetRebootStatus", { priority: "view", retry: false }),
      system: () => api.request("GetSystem", { priority: "view", retry: false }),
    },
    sleep: (ms) =>
      new Promise((done) => {
        deps.timers.setTimeout(done, ms);
      }),
    now: deps.now,
  });

  return {
    system,
    rebootStatus,
    reboot,
    restarts,
    reset() {
      lastUptime = undefined;
      system.reset();
      rebootStatus.reset();
      reboot.reset();
    },
    start() {
      system.start();
      rebootStatus.start();
    },
    stop() {
      system.stop();
      rebootStatus.stop();
    },
  };
}
