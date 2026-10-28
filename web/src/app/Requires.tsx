import type { ComponentChildren } from "preact";
import { useServices } from "./context.ts";
import type { Scope } from "../api/scope.ts";
import styles from "./requires.module.css";

const NEEDS: Record<Scope, string> = {
  read: "Needs read access",
  operate: "Needs operator access",
  configure: "Needs configuration access",
};

/**
 * Shows what needs `scope` only as far as the session has it: disabled
 * with the reason beside it, or not at all. This shapes the page; the device
 * decides what is allowed.
 */
export function Requires({
  scope,
  mode,
  children,
}: {
  scope: Scope;
  mode: "disable" | "hide";
  children: ComponentChildren;
}) {
  const { session } = useServices();
  if (session.can(scope)) return <>{children}</>;
  if (mode === "hide") return null;
  return (
    <fieldset class={styles["fieldset"]} disabled>
      {children}
      <span class={styles["reason"]}>{NEEDS[scope]}</span>
    </fieldset>
  );
}
