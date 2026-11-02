import { BOOT_LINES, ROLES } from "./copy.ts";
import type { InternalFlash, OtadataEntry, RegionInfo } from "./types.ts";

type BootState = NonNullable<InternalFlash["boot"]>;

/**
 * What the next boot does:
 * - `same`: the otadata selects the running slot;
 * - `update`: it selects the other slot, whose record is `new`, an update activated for the next boot;
 * - `fallback`: it selects the other slot, which the bootloader failed to load, so it started the running one;
 * - `none`: no record is valid;
 * - `unknown`: the otadata isn't read (untrusted tier).
 */
export type NextBoot = "same" | "update" | "fallback" | "none" | "unknown";

/** The boot state, and what the page derives from it. */
export interface BootFacts {
  boot: BootState | null;
  next: NextBoot;
  /** Both records of the otadata, in sector order; empty in the untrusted tier. */
  otadata: readonly OtadataEntry[];
  /** The record the bootloader boots: a matching CRC (validity `valid`) and the highest sequence number. */
  booting?: OtadataEntry;
  /**
   * Whether the running slot is the rollback target although the API doesn't say so: during an
   * activated update, while the running app is confirmed.
   */
  derivedRollback: boolean;
}

/** The record with validity `valid` and the highest sequence number, of those that `match`. */
export function bootingRecord(
  otadata: readonly OtadataEntry[],
  match: (entry: OtadataEntry) => boolean = () => true,
): OtadataEntry | undefined {
  let best: OtadataEntry | undefined;
  for (const entry of otadata) {
    if (entry.validity !== "valid" || !match(entry)) continue;
    if (best === undefined || (entry.seq ?? -1) > (best.seq ?? -1)) best = entry;
  }
  return best;
}

export function bootFacts(flash: InternalFlash): BootFacts {
  const boot = flash.boot ?? null;
  const booting = bootingRecord(flash.otadata);
  let next: NextBoot;
  if (boot === null) next = "unknown";
  else if (boot.selected === null || boot.selected === undefined) next = "none";
  else if (boot.selected === boot.running) next = "same";
  else next = booting?.state === "new" ? "update" : "fallback";
  const facts: BootFacts = {
    boot,
    next,
    otadata: flash.otadata,
    derivedRollback: next === "update" && boot?.state === "valid",
  };
  if (booting !== undefined) facts.booting = booting;
  return facts;
}

/** A role chip of an app slot's row. */
export type RoleChip =
  | { role: "running"; label: string }
  /**
   * `plain` on the running slot; `update` (the solid pill) for an activated update; `fallback`
   * (the plain chip with a warning and its tooltip) for a slot the bootloader failed to load.
   */
  | { role: "selected"; label: string; variant: "plain" | "update" | "fallback"; tooltip?: string }
  | { role: "rollback_target"; label: string; derived: boolean }
  /** A role from a newer firmware, as the API names it. */
  | { role: "other"; label: string };

/** Whether a region is an app slot, or the running app outside the slots. */
export function isApp(info: RegionInfo): boolean {
  return info.region === "app_current" || info.region === "app_alternate";
}

/** The region's role chips, in order: Booted, Next boot, Rollback target, then any the page doesn't know. */
export function roleChips(info: RegionInfo, facts: BootFacts): RoleChip[] {
  const roles: readonly string[] = info.roles;
  const running = roles.includes("running");
  const chips: RoleChip[] = [];
  if (running) chips.push({ role: "running", label: ROLES.running });
  if (roles.includes("selected")) {
    if (running) chips.push({ role: "selected", label: ROLES.selected, variant: "plain" });
    else if (facts.next === "update") chips.push({ role: "selected", label: ROLES.selected, variant: "update" });
    else if (facts.next === "fallback") {
      chips.push({ role: "selected", label: ROLES.selected, variant: "fallback", tooltip: ROLES.fallbackTooltip });
    } else chips.push({ role: "selected", label: ROLES.selected, variant: "plain" });
  }
  if (roles.includes("rollback_target")) chips.push({ role: "rollback_target", label: ROLES.rollback, derived: false });
  else if (running && isApp(info) && facts.derivedRollback) {
    chips.push({ role: "rollback_target", label: ROLES.rollback, derived: true });
  }
  for (const role of roles) {
    if (role !== "running" && role !== "selected" && role !== "rollback_target") chips.push({ role: "other", label: role });
  }
  return chips;
}

/** The boot line of an app slot's inspector. */
export function bootLine(info: RegionInfo, chips: readonly RoleChip[], facts: BootFacts): string {
  const has = (role: RoleChip["role"]): boolean => chips.some((chip) => chip.role === role);
  const { boot } = facts;
  const parts: string[] = [];
  if (has("running")) {
    if (boot === null) parts.push(BOOT_LINES.runningNotRead);
    else if (has("selected")) {
      parts.push(BOOT_LINES.runningSelected);
      if (boot.needs_confirmation) parts.push(BOOT_LINES.needsConfirmation);
      else if (boot.state === "valid") parts.push(BOOT_LINES.confirmed);
    } else if (facts.next === "update" && boot.selected) parts.push(BOOT_LINES.runningOtherSelected(boot.selected));
    else if (facts.next === "fallback" && boot.selected) parts.push(BOOT_LINES.fallbackRunning(boot.selected));
    else parts.push(BOOT_LINES.runningNothingSelected);
  } else if (has("selected") && boot !== null) {
    if (facts.next === "fallback") parts.push(BOOT_LINES.fallbackSelected(boot.running));
    else {
      const record = bootingRecord(facts.otadata, (entry) => entry.selects !== null && entry.selects === info.label);
      parts.push(BOOT_LINES.update(record?.state ?? null, boot.running));
    }
  }
  if (has("rollback_target")) parts.push(BOOT_LINES.rollback);
  if (parts.length === 0) parts.push(BOOT_LINES.none);
  return parts.join(" ");
}
