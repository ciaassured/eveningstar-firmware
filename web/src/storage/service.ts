import type { ReadonlySignal } from "@preact/signals";
import { batch, signal } from "@preact/signals";
import type { ApiClient, Result } from "../api/client.ts";
import { callOptions } from "../api/client.ts";
import type { ApiFailure } from "../api/failure.ts";
import { isTransient } from "../api/failure.ts";
import type { Priority, Timers } from "../api/transport.ts";
import { planDownload } from "./plan.ts";
import type { DownloadTarget, PlanResult } from "./plan.ts";
import type { BoardTable, InternalFlash, RegionInfo, StorageDevices } from "./types.ts";

/** How often the flash is read again while the background checks run. */
export const CHECKS_POLL_MS = 3_000;

export interface StorageDeps {
  api: ApiClient;
  timers: Timers;
  now: () => number;
  /** Whether the page is shown and in use: nothing is polled while it isn't. */
  active: ReadonlySignal<boolean>;
  /** How many restarts of the device the page has seen. */
  restarts: ReadonlySignal<number>;
  /** The device's name on networks, read from the device when the page doesn't know it yet. */
  deviceName: () => Promise<string | undefined>;
}

/** What the page knows of the device's storage. */
export interface Storage {
  readonly devices: ReadonlySignal<StorageDevices | undefined>;
  readonly flash: ReadonlySignal<InternalFlash | undefined>;
  readonly regions: ReadonlySignal<readonly RegionInfo[] | undefined>;
  readonly table: ReadonlySignal<BoardTable | undefined>;
  /** When everything was last read, in milliseconds since the epoch. */
  readonly updatedAt: ReadonlySignal<number | undefined>;
  /** Why the latest reload failed; `undefined` if it didn't. The values read before stay. */
  readonly error: ReadonlySignal<ApiFailure | undefined>;
  /** Whether a reload the person asked for is running. */
  readonly refreshing: ReadonlySignal<boolean>;
  /** The view's loads, in order, for its `LoadGate`. */
  loaders(group: string): (() => Promise<Result<unknown>>)[];
  /** Reads everything again. */
  refresh(priority: Priority): Promise<Result<unknown>>;
  /**
   * Keeps the values current while a view shows them: reads the flash every few
   * seconds while the background checks run, and everything again when the
   * device restarts. Returns a function that stops it.
   */
  watch(): () => void;
  /** Reads the flash again and plans the download against it. */
  prepare(target: DownloadTarget): Promise<PlanResult>;
  /** Forgets everything read, such as when another person signs in. */
  reset(): void;
}

export function createStorage(deps: StorageDeps): Storage {
  const { api, timers } = deps;
  const devices = signal<StorageDevices | undefined>(undefined);
  const flash = signal<InternalFlash | undefined>(undefined);
  const regions = signal<readonly RegionInfo[] | undefined>(undefined);
  const table = signal<BoardTable | undefined>(undefined);
  const updatedAt = signal<number | undefined>(undefined);
  const error = signal<ApiFailure | undefined>(undefined);
  const refreshing = signal(false);

  function noted<T>(result: Result<T>, store: (value: T) => void): Result<T> {
    if (result.ok) {
      batch(() => {
        store(result.value);
        updatedAt.value = deps.now();
      });
    }
    return result;
  }

  const loadDevices = async (priority: Priority, group?: string) =>
    noted(await api.request("GetStorage", callOptions(priority, group)), (value) => {
      devices.value = value;
    });
  const loadFlash = async (priority: Priority, group?: string) =>
    noted(await api.request("GetInternalFlash", callOptions(priority, group)), (value) => {
      flash.value = value;
    });
  const loadRegions = async (priority: Priority, group?: string) =>
    noted(await api.request("GetRegions", callOptions(priority, group)), (value) => {
      regions.value = value.regions;
    });
  const loadTable = async (priority: Priority, group?: string) =>
    noted(await api.request("GetPartitionTable", callOptions(priority, group)), (value) => {
      table.value = value;
    });

  /** Runs the loads in order and stops at the first failure, noting it as the reload's error. */
  async function chain(loads: readonly (() => Promise<Result<unknown>>)[]): Promise<Result<unknown>> {
    let last: Result<unknown> | undefined;
    for (const load of loads) {
      last = await load();
      if (!last.ok) {
        if (last.failure.kind !== "aborted") error.value = last.failure;
        return last;
      }
    }
    error.value = undefined;
    return last ?? { ok: true, value: undefined, context: { method: "GET", path: "", attempts: 0, at: deps.now() } };
  }

  const refresh = async (priority: Priority): Promise<Result<unknown>> => {
    refreshing.value = priority === "action";
    try {
      return await chain([
        () => loadDevices(priority),
        () => loadFlash(priority),
        () => loadRegions(priority),
        () => loadTable(priority),
      ]);
    } finally {
      refreshing.value = false;
    }
  };

  return {
    devices,
    flash,
    regions,
    table,
    updatedAt,
    error,
    refreshing,
    loaders(group) {
      return [
        () => loadDevices("view", group),
        () => loadFlash("view", group),
        () => loadRegions("view", group),
        () => loadTable("view", group),
      ];
    },
    refresh,
    watch() {
      let timer: unknown;
      let polling = false;
      let stopped = false;
      const schedule = (): void => {
        timers.clearTimeout(timer);
        if (stopped || polling || flash.peek()?.checks.status !== "running" || !deps.active.peek()) return;
        timer = timers.setTimeout(() => {
          polling = true;
          void chain([() => loadFlash("poll"), () => loadRegions("poll")]).finally(() => {
            polling = false;
            schedule();
          });
        }, CHECKS_POLL_MS);
      };
      const stopFlash = flash.subscribe(schedule);
      const stopActive = deps.active.subscribe(schedule);
      const seen = deps.restarts.peek();
      const stopRestarts = deps.restarts.subscribe((count) => {
        if (count !== seen) void refresh("view");
      });
      return () => {
        stopped = true;
        timers.clearTimeout(timer);
        stopFlash();
        stopActive();
        stopRestarts();
      };
    },
    async prepare(target) {
      const read = await chain([() => loadFlash("action"), () => loadRegions("action")]);
      const current = flash.peek();
      const listed = regions.peek();
      if (!read.ok || current === undefined || listed === undefined) {
        return {
          ok: false,
          reason: "The device's storage couldn't be read to plan the download.",
          transient: read.ok || isTransient(read.failure),
        };
      }
      const device = (await deps.deviceName()) ?? "eveningstar";
      // The board's table changes only across a restart, which reads everything again.
      return planDownload(target, { flash: current, regions: listed, device, table: table.peek() });
    },
    reset() {
      batch(() => {
        devices.value = undefined;
        flash.value = undefined;
        regions.value = undefined;
        table.value = undefined;
        updatedAt.value = undefined;
        error.value = undefined;
      });
    },
  };
}
