import assert from "node:assert/strict";
import { test } from "node:test";
import { signal } from "@preact/signals";
import type { Scope } from "../api/scope.ts";
import { NoAuth } from "./auth.ts";
import type { AuthSession, AuthState } from "./auth.ts";
import { createSession } from "./session.ts";

test("with no authentication, everyone may do everything", () => {
  const session = createSession(new NoAuth());
  assert.equal(session.can("read"), true);
  assert.equal(session.can("operate"), true);
  assert.equal(session.can("configure"), true);
});

test("signing in is unsupported without authentication", async () => {
  const auth = new NoAuth();
  assert.deepEqual(await auth.login({ user: "a", secret: "b" }), { ok: false, reason: "unsupported" });
  assert.equal(await auth.onUnauthorized(), "login-required");
  assert.deepEqual(auth.decorate({ method: "GET" }), { method: "GET" });
});

function signedIn(scopes: readonly Scope[]) {
  const state = signal<AuthState>({ kind: "anonymous" });
  const auth: AuthSession = {
    state,
    anonymousScopes: ["read"],
    decorate: (init) => init,
    onUnauthorized: () => Promise.resolve("login-required"),
    login: () => Promise.resolve({ ok: true }),
    logout: () => Promise.resolve(),
  };
  return {
    state,
    session: createSession(auth),
    signIn: (user: string) => {
      state.value = { kind: "authenticated", user, scopes };
    },
  };
}

test("an anonymous session has the provider's anonymous scopes, a signed-in one its own", () => {
  const { session, signIn } = signedIn(["read", "operate"]);
  assert.equal(session.can("read"), true);
  assert.equal(session.can("operate"), false);
  signIn("operator");
  assert.equal(session.can("operate"), true);
  assert.equal(session.can("configure"), false);
});

test("listeners hear when the person signs out or another person signs in, not on first sign-in", () => {
  const { session, state, signIn } = signedIn(["read"]);
  let changes = 0;
  const stop = session.onUserChange(() => {
    changes += 1;
  });
  assert.equal(changes, 0);
  signIn("alice");
  assert.equal(changes, 1);
  signIn("alice");
  assert.equal(changes, 1);
  signIn("bob");
  assert.equal(changes, 2);
  state.value = { kind: "expired" };
  assert.equal(changes, 3);
  stop();
  signIn("carol");
  assert.equal(changes, 3);
});
