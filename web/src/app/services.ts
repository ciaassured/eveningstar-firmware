import type { ReadonlySignal } from "@preact/signals";
import { createApiClient } from "../api/client.ts";
import type { ApiClient } from "../api/client.ts";
import { Transport } from "../api/transport.ts";
import type { Timers } from "../api/transport.ts";
import { createDevice } from "../device/device.ts";
import type { Device } from "../device/device.ts";
import { createActivity } from "../live/activity.ts";
import type { Activity, ActivityEnvironment } from "../live/activity.ts";
import { createConnection } from "../live/connection.ts";
import type { Connection } from "../live/connection.ts";
import { createTicker } from "../live/ticker.ts";
import { NoAuth } from "../session/auth.ts";
import { createSession } from "../session/session.ts";
import type { Session } from "../session/session.ts";
import { createRouter } from "./router.ts";
import type { Router, RouterEnvironment } from "./router.ts";

/** What the page needs of the browser. */
export interface Environment {
  fetch: (url: string, init: RequestInit) => Promise<Response>;
  timers: Timers;
  random: () => number;
  now: () => number;
  activity: ActivityEnvironment;
  router: RouterEnvironment;
}

/** Everything the views share. */
export interface Services {
  readonly transport: Transport;
  readonly api: ApiClient;
  readonly session: Session;
  readonly router: Router;
  readonly connection: Connection;
  readonly activity: Activity;
  readonly device: Device;
  readonly timers: Timers;
  /** The time, refreshed every second while the page is in use. */
  readonly now: ReadonlySignal<number>;
}

export function createServices(environment: Environment): Services {
  const auth = new NoAuth();
  const session = createSession(auth);
  const router = createRouter(environment.router);
  const connection = createConnection(environment.now);
  const activity = createActivity(environment.activity);

  const transport = new Transport({
    fetch: environment.fetch,
    timers: environment.timers,
    random: environment.random,
    now: environment.now,
    auth,
    onLoginRequired: () => {
      router.requireLogin();
    },
    onAttempt: (reachable) => {
      connection.report(reachable);
    },
  });
  const api = createApiClient(transport);

  const device = createDevice({
    api,
    timers: environment.timers,
    now: environment.now,
    active: activity.active,
  });
  session.onUserChange(() => {
    device.reset();
  });

  return {
    transport,
    api,
    session,
    router,
    connection,
    activity,
    device,
    timers: environment.timers,
    now: createTicker(environment.timers, environment.now, activity.active),
  };
}
