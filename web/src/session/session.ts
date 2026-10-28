import type { ReadonlySignal } from "@preact/signals";
import { computed } from "@preact/signals";
import type { Capability } from "../lib/capability.ts";
import type { AuthSession, AuthState } from "./auth.ts";

/** Who is using the page, and what they may do. */
export interface Session {
  readonly auth: AuthSession;
  readonly capabilities: ReadonlySignal<ReadonlySet<Capability>>;
  /** Whether the session may do something that needs `capability`. The device enforces it; this only shapes the page. */
  can(capability: Capability): boolean;
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
  const capabilities = computed<ReadonlySet<Capability>>(() => {
    const state = auth.state.value;
    return new Set(state.kind === "authenticated" ? state.capabilities : auth.anonymousCapabilities);
  });

  return {
    auth,
    capabilities,
    can: (capability) => capabilities.value.has(capability),
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
