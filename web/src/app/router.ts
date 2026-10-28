import type { ReadonlySignal } from "@preact/signals";
import { signal } from "@preact/signals";

export type Route =
  | { name: "dashboard" }
  | { name: "system" }
  | { name: "login" }
  | { name: "not-found"; path: string };

/** The routes with a place in the navigation, in order. */
export const NAVIGATION: readonly { route: Route["name"]; path: string; label: string }[] = [
  { route: "dashboard", path: "/", label: "Dashboard" },
  { route: "system", path: "/system", label: "System" },
];

/** The route a URL fragment such as `#/system` names. */
export function parseRoute(hash: string): Route {
  const path = hash.replace(/^#/, "").split("?")[0] ?? "";
  const normal = path === "" ? "/" : path.length > 1 ? path.replace(/\/+$/, "") : path;
  switch (normal) {
    case "/":
      return { name: "dashboard" };
    case "/system":
      return { name: "system" };
    case "/login":
      return { name: "login" };
    default:
      return { name: "not-found", path: normal };
  }
}

/** The URL fragment for the route with the given path. */
export function toHash(path: string): string {
  return `#${path}`;
}

/** The path of a route, for building its URL fragment. */
export function pathOf(route: Route): string {
  switch (route.name) {
    case "dashboard":
      return "/";
    case "system":
      return "/system";
    case "login":
      return "/login";
    case "not-found":
      return route.path;
  }
}

/** What the router reads of the browser. */
export interface RouterEnvironment {
  hash(): string;
  setHash(hash: string): void;
  /** Calls `listener` when the fragment changes. Returns a function that stops the calls. */
  onHashChange(listener: () => void): () => void;
}

/** Decides whether the page may be left for a route. */
export type Guard = (to: Route) => boolean;

export interface Router {
  readonly route: ReadonlySignal<Route>;
  navigate(path: string): void;
  /** Installs the guard asked before every navigation; `undefined` removes it. */
  setGuard(guard: Guard | undefined): void;
  /** Goes to the login route, remembering where the person was. */
  requireLogin(): void;
  /** Goes back to where the person was before the login route, or the dashboard. */
  returnFromLogin(): void;
  dispose(): void;
}

export function createRouter(environment: RouterEnvironment): Router {
  const route = signal<Route>(parseRoute(environment.hash()));
  let guard: Guard | undefined;
  let before: string | undefined;

  const stop = environment.onHashChange(() => {
    const next = parseRoute(environment.hash());
    if (guard !== undefined && !guard(next)) {
      environment.setHash(toHash(pathOf(route.peek())));
      return;
    }
    route.value = next;
  });

  return {
    route,
    navigate(path) {
      environment.setHash(toHash(path));
    },
    setGuard(next) {
      guard = next;
    },
    requireLogin() {
      const current = route.peek();
      if (current.name === "login") return;
      before = pathOf(current);
      environment.setHash(toHash("/login"));
    },
    returnFromLogin() {
      environment.setHash(toHash(before ?? "/"));
      before = undefined;
    },
    dispose: stop,
  };
}
