import type { ComponentChildren } from "preact";
import { useServices } from "./context.ts";
import type { Capability } from "../api/capability.ts";
import styles from "./requires.module.css";

const NEEDS: Record<Capability, string> = {
  read: "Needs read access",
  operate: "Needs operator access",
  configure: "Needs configuration access",
};

/**
 * Shows what needs `capability` only as far as the session has it: disabled
 * with the reason beside it, or not at all. This shapes the page; the device
 * decides what is allowed.
 */
export function Requires({
  capability,
  mode,
  children,
}: {
  capability: Capability;
  mode: "disable" | "hide";
  children: ComponentChildren;
}) {
  const { session } = useServices();
  if (session.can(capability)) return <>{children}</>;
  if (mode === "hide") return null;
  return (
    <fieldset class={styles["fieldset"]} disabled>
      {children}
      <span class={styles["reason"]}>{NEEDS[capability]}</span>
    </fieldset>
  );
}
