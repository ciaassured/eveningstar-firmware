/**
 * What a session may do: read the device's state, take operational actions
 * such as a reboot, or edit the persistent configuration.
 */
export type Capability = "read" | "operate" | "configure";

export const CAPABILITIES: readonly Capability[] = ["read", "operate", "configure"];
