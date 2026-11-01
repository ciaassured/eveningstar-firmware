import { formatBytes, formatCountdown, formatPercent } from "../lib/format.ts";
import type { StatusLevel } from "../ui/StatusIcon.tsx";
import type { DownloadEntry, EntryAction } from "./service.ts";

/** A download as the list shows it. */
export interface EntryView {
  id: number;
  label: string;
  filename?: string;
  level: StatusLevel;
  status: string;
  /** Detail under the status, such as why it started again. */
  detail?: string;
  /** The share done, while downloading with a known total. */
  progress?: number;
  /** The SHA-256 to compare the saved file with by hand, and the command that does. */
  expected?: { sha256: string; command: string };
  actions: EntryAction[];
}

export const ACTION_LABELS: Record<EntryAction, string> = {
  cancel: "Cancel",
  up: "Move up",
  down: "Move down",
  resume: "Resume",
  dismiss: "Dismiss",
  "save-anyway": "Save anyway",
};

/** The command that hashes the image at the start of the saved file. */
function hashCommand(filename: string, bytes: number, whole: boolean): string {
  return whole ? `sha256sum ${filename}` : `head -c ${bytes} ${filename} | sha256sum`;
}

/** What the list shows of an entry, at the time `now`. */
export function entryView(
  entry: DownloadEntry,
  options: { now: number; verifies: boolean; position?: number; first?: boolean; last?: boolean },
): EntryView {
  const view: EntryView = { id: entry.id, label: entry.label, level: "off", status: "", actions: [] };
  const { plan, state } = entry;
  if (plan !== undefined) view.filename = plan.filename;
  if (entry.note !== undefined) view.detail = entry.note;
  switch (state.kind) {
    case "queued":
      view.level = "standby";
      view.status = options.position === undefined ? "Queued" : `Queued, ${ordinal(options.position)} in line`;
      view.actions = [
        ...(options.first === true ? [] : (["up"] as const)),
        ...(options.last === true ? [] : (["down"] as const)),
        "cancel",
      ];
      break;
    case "waiting":
      view.level = "standby";
      view.status = `Waiting: another client is downloading. Asking again in ${formatCountdown((state.until - options.now) / 1_000)}`;
      view.detail = `Waiting for ${formatCountdown((options.now - state.since) / 1_000)}`;
      view.actions = ["cancel"];
      break;
    case "downloading":
      view.actions = ["cancel"];
      if (state.total === undefined) {
        view.level = "standby";
        view.status = "Preparing";
        break;
      }
      view.level = state.retrying ? "caution" : "normal";
      view.progress = state.total === 0 ? 1 : state.received / state.total;
      view.status = `${state.retrying ? "Resuming" : "Downloading"} ${formatPercent(view.progress)}: ${formatBytes(state.received)} of ${formatBytes(state.total)}`;
      break;
    case "verifying":
      view.level = "normal";
      view.status = "Checking the SHA-256";
      view.actions = ["cancel"];
      break;
    case "done":
      view.level = state.verified === "mismatch" ? "serious" : "normal";
      view.status =
        state.verified === "checked"
          ? "Saved; the SHA-256 matches"
          : state.verified === "mismatch"
            ? "Saved as unverified"
            : "Saved";
      view.actions = ["dismiss"];
      break;
    case "failed":
      view.level = "serious";
      view.status = `Failed: ${state.reason}`;
      view.actions = [
        ...(state.resumable ? (["resume"] as const) : []),
        ...(state.mismatch === undefined ? [] : (["save-anyway"] as const)),
        "dismiss",
      ];
      if (state.mismatch !== undefined) {
        view.detail = `Expected ${state.mismatch.expected}, got ${state.mismatch.actual}`;
      }
      break;
    case "cancelled":
      view.level = "off";
      view.status = "Cancelled";
      view.actions = ["dismiss"];
      break;
  }
  // Without a secure context the page can't hash, so the person can.
  if (plan?.expected !== undefined && !options.verifies && state.kind !== "cancelled") {
    view.expected = {
      sha256: plan.expected.sha256,
      command: hashCommand(plan.filename, plan.expected.bytes, plan.expected.bytes === plan.size),
    };
  }
  return view;
}

/** Every entry as the list shows it, with each queued entry's place in line. */
export function entryViews(entries: readonly DownloadEntry[], now: number, verifies: boolean): EntryView[] {
  const queued = entries.filter((entry) => entry.state.kind === "queued").map((entry) => entry.id);
  return entries.map((entry) => {
    const position = queued.indexOf(entry.id);
    return position < 0
      ? entryView(entry, { now, verifies })
      : entryView(entry, {
          now,
          verifies,
          position: position + 1,
          first: position === 0,
          last: position === queued.length - 1,
        });
  });
}

/** What the header's tray says: the running download's progress and how many wait. */
export function traySummary(entries: readonly DownloadEntry[], now: number): string {
  const queued = entries.filter((entry) => entry.state.kind === "queued").length;
  const running = entries.find((entry) => ["waiting", "downloading", "verifying"].includes(entry.state.kind));
  const parts: string[] = [];
  if (running !== undefined) {
    const { state } = running;
    if (state.kind === "waiting") parts.push(`Waiting ${formatCountdown((state.until - now) / 1_000)}`);
    else if (state.kind === "verifying") parts.push("Checking");
    else if (state.kind === "downloading" && state.total !== undefined && state.total > 0) {
      parts.push(formatPercent(state.received / state.total));
    } else parts.push("Preparing");
  }
  if (queued > 0) parts.push(`${queued} queued`);
  if (parts.length === 0) {
    const failed = entries.filter((entry) => entry.state.kind === "failed").length;
    return failed > 0 ? `${failed} failed` : `${entries.length} finished`;
  }
  return parts.join(" · ");
}

/** A word or two for an entry beside its region's button; `undefined` when there is nothing to say. */
export function shortStatus(entry: DownloadEntry | undefined): { level: StatusLevel; label: string } | undefined {
  if (entry === undefined) return undefined;
  const { state } = entry;
  switch (state.kind) {
    case "queued":
      return { level: "standby", label: "Queued" };
    case "waiting":
      return { level: "standby", label: "Waiting" };
    case "downloading":
      return state.total === undefined || state.total === 0
        ? { level: "standby", label: "Preparing" }
        : { level: state.retrying ? "caution" : "normal", label: formatPercent(state.received / state.total) };
    case "verifying":
      return { level: "normal", label: "Checking" };
    case "done":
      return state.verified === "mismatch" ? { level: "serious", label: "Unverified" } : { level: "normal", label: "Saved" };
    case "failed":
      return { level: "serious", label: "Failed" };
    case "cancelled":
      return undefined;
  }
}

function ordinal(n: number): string {
  const tens = n % 100;
  const suffix = tens >= 11 && tens <= 13 ? "th" : (["th", "st", "nd", "rd"][n % 10] ?? "th");
  return `${n}${suffix}`;
}
