import { TRAY, regionIdentity } from "./copy.ts";
import type { DownloadPlan, DownloadTarget } from "./plan.ts";
import type { RegionInfo } from "./types.ts";

/** What a download in the tray is, the hash it shows and its note. */
export interface DownloadSubject {
  /** What it is, such as "App image, ota_0". */
  what: string;
  /** The region's title and subtitle, for the "From" fact. */
  title: string;
  subtitle: string;
  /** The hash field, when the download has one and the plan holds its value. */
  hash?: { label: string; text: string; value: string };
  note?: string;
}

/** What a download is, from its target, the regions and, once planned, its plan. */
export function downloadSubject(
  target: DownloadTarget,
  regions: readonly RegionInfo[],
  plan?: DownloadPlan,
): DownloadSubject {
  const info = regions.find((region) => region.region === target.region);
  const identity = regionIdentity(target.region, info?.label ?? null);
  const base = { title: identity.title, subtitle: identity.subtitle };
  switch (target.what.kind) {
    case "image": {
      const subject: DownloadSubject = {
        ...base,
        what: target.region === "bootloader" ? TRAY.bootloaderImage : TRAY.appImage(identity.title),
      };
      if (plan?.expected !== undefined) {
        subject.hash = { label: TRAY.imageHash, text: TRAY.imageHashText, value: plan.expected.sha256 };
      }
      return subject;
    }
    case "range":
      return {
        ...base,
        what: TRAY.range(identity.title, target.what.span.first, target.what.span.last + 1),
        note: TRAY.noHashNote,
      };
    case "whole":
      break;
  }
  if (target.region === "partition_table") {
    const subject: DownloadSubject = { ...base, what: TRAY.table, note: TRAY.tableNote };
    if (plan?.md5 !== undefined) subject.hash = { label: TRAY.tableHash, text: TRAY.tableHashText, value: plan.md5 };
    return subject;
  }
  if (target.region === "otadata") return { ...base, what: TRAY.otadata, note: TRAY.noHashNote };
  return { ...base, what: TRAY.whole(identity.title), note: TRAY.noHashNote };
}
