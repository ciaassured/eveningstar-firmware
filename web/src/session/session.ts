import type { ReadonlySignal } from "@preact/signals";
import { computed } from "@preact/signals";
import type { Scope } from "../api/scope.ts";
import type { AuthSession, AuthState } from "./auth.ts";

/** Who is using the page, and what they may do. */
export interface Session {
  readonly auth: AuthSession;
  readonly scopes: ReadonlySignal<ReadonlySet<Scope>>;
  /** Whether the session may do something that needs `scope`. The device enforces it; this only shapes the page. */
  can(scope: Scope): boolean;
  /**
   * Calls `listener` when the person signs out or another person signs in, so
   * that nothing one session fetched is shown to the next. Returns a function
   * that stops the calls.
   */
  onUserChange(listener: () => void): () => void;
}

function identity(state: AuthState): string | undefined {
  return state.kind === "authenticated" ? state.user : undefined;
}

export function createSession(auth: AuthSession): Session {
  const scopes = computed<ReadonlySet<Scope>>(() => {
    const state = auth.state.value;
    return new Set(state.kind === "authenticated" ? state.scopes : auth.anonymousScopes);
  });

  return {
    auth,
    scopes,
    can: (scope) => scopes.value.has(scope),
    onUserChange(listener) {
      let previous = identity(auth.state.peek());
      return auth.state.subscribe((state) => {
        const current = identity(state);
        if (current === previous) return;
        previous = current;
        listener();
      });
    },
  };
}
