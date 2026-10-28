import type { ReadonlySignal } from "@preact/signals";
import { signal } from "@preact/signals";
import type { AuthProvider } from "../api/auth.ts";
import type { Capability } from "../lib/capability.ts";
import { CAPABILITIES } from "../lib/capability.ts";

export type AuthState =
  | { kind: "anonymous" }
  | {
      kind: "authenticated";
      user: string;
      capabilities: readonly Capability[];
      /** Milliseconds since the epoch. */
      expiresAt?: number;
    }
  | { kind: "expired" };

export interface LoginInput {
  user: string;
  secret: string;
}

export type LoginResult = { ok: true } | { ok: false; reason: "unsupported" | "rejected" | "unreachable" };

/** An auth provider the person can sign in to and out of. */
export interface AuthSession extends AuthProvider {
  readonly state: ReadonlySignal<AuthState>;
  /** What a session that hasn't signed in may do. */
  readonly anonymousCapabilities: readonly Capability[];
  login(input: LoginInput): Promise<LoginResult>;
  logout(): Promise<void>;
}

/**
 * The device has no authentication yet: nobody signs in, and everyone may do
 * everything.
 */
export class NoAuth implements AuthSession {
  readonly state = signal<AuthState>({ kind: "anonymous" });
  readonly anonymousCapabilities = CAPABILITIES;

  decorate(init: RequestInit): RequestInit {
    return init;
  }

  onUnauthorized(): Promise<"retry" | "login-required"> {
    return Promise.resolve("login-required");
  }

  login(_input: LoginInput): Promise<LoginResult> {
    return Promise.resolve({ ok: false, reason: "unsupported" });
  }

  logout(): Promise<void> {
    return Promise.resolve();
  }
}
