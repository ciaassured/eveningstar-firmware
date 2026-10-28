import type { RebootHolder, RebootMode, RebootReason } from "./types.ts";

/**
 * What the device says in words. The values are the ones the API names today;
 * a device on newer firmware may send others, which are shown as they came.
 */
export function rebootReasonLabel(reason: RebootReason | (string & {})): string {
  switch (reason) {
    case "link_down":
      return "The Ethernet link stayed down";
    case "api":
      return "Requested over the API";
    default:
      return `Reason "${reason}"`;
  }
}

export function rebootHolderLabel(holder: RebootHolder | (string & {})): string {
  switch (holder) {
    case "http":
      return "A web request in progress";
    default:
      return `Holder "${holder}"`;
  }
}

export function rebootModeLabel(mode: RebootMode): string {
  switch (mode) {
    case "graceful":
      return "Wait for work in progress to finish";
    case "immediate":
      return "Restart now";
  }
}
