import { formatClock, formatCountdown, formatPercent, formatSize, formatSpan } from "../lib/format.ts";
import { NONE } from "../storage/copy.ts";
import { unverifiedFilename } from "../storage/filenames.ts";
import type { DownloadPlan, DownloadTarget } from "../storage/plan.ts";
import { downloadSubject } from "../storage/tray.ts";
import type { RegionInfo } from "../storage/types.ts";
import type { DownloadStatus, TrayAction, TrayItem } from "../ui/DownloadTray.tsx";
import type { DownloadEntry, EntryState } from "./service.ts";

const WORDS: Record<DownloadStatus, string> = {
  pending: "Pending",
  downloading: "Downloading",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
};

/** The tray's status of a download state. */
export function statusOf(state: EntryState): DownloadStatus {
  switch (state.kind) {
    case "queued":
    case "waiting":
      return "pending";
    case "downloading":
    case "verifying":
      return "downloading";
    case "done":
      return state.verified === "mismatch" ? "failed" : "completed";
    case "failed":
      return "failed";
    case "cancelled":
      return "cancelled";
  }
}

/** Whether a status counts as active: pending or downloading. */
export function isActive(status: DownloadStatus): boolean {
  return status === "pending" || status === "downloading";
}

/** How long ago something was queued: `just now`, `<n> s ago`, `<n> min ago`, `<n> h ago`. */
export function queuedAge(ageMs: number): string {
  const s = Math.floor(Math.max(0, ageMs) / 1_000);
  if (s < 10) return "just now";
  if (s < 60) return `${s} s ago`;
  if (s < 3_600) return `${Math.floor(s / 60)} min ago`;
  return `${Math.floor(s / 3_600)} h ago`;
}

/** What the tray needs besides the entries. */
export interface ViewFacts {
  now: number;
  regions: readonly RegionInfo[];
  /** A plan for an entry that has none yet, from what the device reported last. */
  preview: (target: DownloadTarget) => DownloadPlan | undefined;
}

function share(done: number, total: number | undefined): number {
  return total === undefined || total <= 0 ? 0 : done / total;
}

/** The amount, the "Progress" fact and the share of a download, from its state and size. */
function amounts(state: EntryState, size: number | undefined): { amount: string; progress: string; share?: number } {
  const sized = size === undefined ? NONE : formatSize(size);
  const of = (done: number): string => (size === undefined ? formatSize(done) : `${formatSize(done)} of ${sized}`);
  switch (state.kind) {
    case "queued":
      return { amount: sized, progress: sized };
    case "waiting":
      return { amount: "Waiting", progress: sized };
    case "downloading":
      if (state.total === undefined) return { amount: "Preparing", progress: sized };
      return { amount: formatPercent(share(state.received, state.total)), progress: of(state.received), share: share(state.received, state.total) };
    case "verifying":
      return { amount: "Checking", progress: of(size ?? 0), share: 1 };
    case "done":
      return state.verified === "mismatch"
        ? { amount: "Unverified", progress: of(size ?? 0) }
        : { amount: sized, progress: `${sized} saved` };
    case "failed":
      return { amount: `at ${formatPercent(share(state.received, size))}`, progress: of(state.received) };
    case "cancelled":
      return { amount: sized, progress: sized };
  }
}

/** The note a state adds, if any. */
function stateNote(state: EntryState, now: number): string | undefined {
  if (state.kind === "waiting") {
    return `Another client is downloading from the device. Asking again in ${formatCountdown((state.until - now) / 1_000)}.`;
  }
  if (state.kind === "downloading" && state.retrying) {
    return `The connection broke; the download resumes from ${formatSize(state.received)}.`;
  }
  if (state.kind === "done" && state.verified === "mismatch") {
    return "Saved as unverified: the SHA-256 of the received bytes does not match the device's.";
  }
  return undefined;
}

function actionsOf(state: EntryState, status: DownloadStatus): TrayAction[] {
  if (isActive(status)) return ["cancel"];
  if (status === "completed") return ["remove"];
  return state.kind === "failed" && state.mismatch !== undefined ? ["retry", "save-anyway", "remove"] : ["retry", "remove"];
}

/** A download as the tray shows it. */
export function trayItem(entry: DownloadEntry, facts: ViewFacts): TrayItem {
  const { state, target } = entry;
  const plan = entry.plan ?? facts.preview(target);
  const status = statusOf(state);
  const subject = downloadSubject(target, facts.regions, plan);
  const { amount, progress, share: done } = amounts(state, plan?.size);
  const info = facts.regions.find((region) => region.region === target.region);
  const notes = [stateNote(state, facts.now), entry.note, subject.note].filter((note) => note !== undefined);
  let filename = plan?.filename ?? subject.what;
  if (state.kind === "done" && state.verified === "mismatch") filename = unverifiedFilename(filename);
  const item: TrayItem = {
    id: entry.id,
    status,
    word: WORDS[status],
    amount,
    filename,
    what: subject.what,
    queued: `${formatClock(entry.queuedAt)}, ${queuedAge(facts.now - entry.queuedAt)}`,
    from: { code: subject.code, colour: subject.colour, title: subject.title, subtitle: subject.subtitle },
    progress,
    notes,
    actions: actionsOf(state, status),
  };
  if (done !== undefined) item.share = done;
  if (state.kind === "failed") item.reason = state.reason;
  if (info !== undefined && plan !== undefined) item.span = formatSpan(info.offset + (plan.span?.first ?? 0), plan.size);
  if (subject.hash !== undefined) item.hash = subject.hash;
  return item;
}

/** Every download, newest first: the queue holds them oldest first. */
export function trayItems(entries: readonly DownloadEntry[], facts: ViewFacts): TrayItem[] {
  return entries.map((entry) => trayItem(entry, facts)).reverse();
}

/** The tray's count: `<a> active, <f> finished`. */
export function trayCount(items: readonly TrayItem[]): string {
  const active = items.filter((item) => isActive(item.status)).length;
  return `${active} active, ${items.length - active} finished`;
}

/** The entries "Clear finished" removes: completed and cancelled. Failed ones keep their bytes for a retry. */
export function finishedIds(items: readonly TrayItem[]): number[] {
  return items.filter((item) => item.status === "completed" || item.status === "cancelled").map((item) => item.id);
}

/** The header button: how many are active, the running download's share, and its accessible name. */
export function buttonView(items: readonly TrayItem[]): { active: number; share: number; label: string } {
  const active = items.filter((item) => isActive(item.status)).length;
  const running = items.find((item) => item.status === "downloading");
  const runningShare = running?.share;
  if (active === 0) return { active, share: 0, label: "Downloads, none active" };
  if (running === undefined || runningShare === undefined) return { active, share: 0, label: `Downloads, ${active} active` };
  return {
    active,
    share: runningShare,
    label: `Downloads, ${active} active, ${formatPercent(runningShare)} of ${running.filename}`,
  };
}
