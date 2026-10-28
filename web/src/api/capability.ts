import type { components } from "./generated/v1.d.ts";

/**
 * What a session needs to be allowed to call an operation, as the API
 * declares them: reading the device's state, operational actions such as a
 * reboot, or editing the persistent configuration.
 */
export type Capability = components["schemas"]["Capability"];

// A capability the API adds is missing here until it is written, and then this doesn't type-check.
const EVERY: Record<Capability, true> = { read: true, operate: true, configure: true };

export const CAPABILITIES = Object.keys(EVERY) as readonly Capability[];
