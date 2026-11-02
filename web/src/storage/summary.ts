import { REASONS, SUMMARY, deviceName } from "./copy.ts";
import type { Level } from "./copy.ts";
import type { StorageModel } from "./items.ts";

/** A line of the list's summary header. */
export interface SummaryLine {
  /** The icon's level; `muted` for a line without an icon, in the muted colour. */
  level: Level | "muted";
  text: string;
}

/** The list's summary header. */
export interface SummaryView {
  /** "8 MiB flash", or with the note that the chip's size isn't known. */
  title: string;
  /** The status line. */
  status: { level: Level; text: string };
  /** The lines under it: images that fail verification, a fallback, other storage devices. */
  lines: SummaryLine[];
}

function sentences(...parts: string[]): string {
  return parts.filter((part) => part !== "").join(" ");
}

/** The status line of the summary header. */
export function summaryStatus(model: StorageModel): { level: Level; text: string } {
  const { consequence, boot } = model;
  if (model.comparison === "no-slot") return { level: "warn", text: sentences(REASONS.notASlot, consequence) };
  if (model.comparison === "unknown") {
    return { level: "warn", text: sentences(REASONS.tableUnknown(model.search), consequence) };
  }
  const notExpected = model.items.filter((item) => item.status.level !== "ok");
  if (notExpected.length > 0) {
    return {
      level: "warn",
      text: sentences(SUMMARY.notExpected(notExpected.map((item) => item.title)), consequence),
    };
  }
  const update = boot.next === "update" && boot.boot?.selected ? SUMMARY.update(boot.boot.selected) : "";
  return { level: consequence === "" ? "ok" : "warn", text: sentences(SUMMARY.matches, update, consequence) };
}

/** The summary header of the region list. */
export function summaryView(model: StorageModel): SummaryView {
  const { flash, regions, devices } = model.input;
  const lines: SummaryLine[] = [];
  for (const app of flash.apps) {
    if (app.image.status !== "invalid") continue;
    const label = regions.find((info) => info.region === app.region)?.label ?? app.slot ?? app.region;
    lines.push({ level: "bad", text: SUMMARY.invalidImage(label, app.image.error ?? null) });
  }
  const boot = model.boot.boot;
  if (model.boot.next === "fallback" && boot?.selected) {
    lines.push({ level: "warn", text: SUMMARY.fallback(boot.selected, boot.running) });
  }
  for (const device of devices?.devices ?? []) {
    if (device.id === "internal") continue;
    lines.push({ level: "muted", text: SUMMARY.device(deviceName(device.id), device.status) });
  }
  return { title: SUMMARY.flash(model.flashSize, model.sizeKnown), status: summaryStatus(model), lines };
}
