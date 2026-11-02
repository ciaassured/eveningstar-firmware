import { downloadFilename } from "./filenames.ts";
import { lockReason } from "./labels.ts";
import type { BoardTable, InternalFlash, Region, RegionInfo, SlotName, Span } from "./types.ts";

/** What of a region to download. */
export type DownloadWhat = { kind: "whole" } | { kind: "image" } | { kind: "range"; span: Span };

/** A download as the person asked for it, before it is planned against what the device reports. */
export interface DownloadTarget {
  region: Region;
  what: DownloadWhat;
}

/** A digest the first `bytes` bytes of the download hash to. */
export interface DigestCheck {
  sha256: string;
  bytes: number;
  /** Which of the image's digests it is. */
  against: "file_sha256" | "digest";
}

/** A download planned against what the device reported last. */
export interface DownloadPlan {
  region: Region;
  slot?: SlotName | null;
  /** The part of the region to ask for; the whole region when `undefined`. */
  span?: Span;
  /** The bytes expected. */
  size: number;
  filename: string;
  /** What the page hashes the download against, where it can hash. */
  check?: DigestCheck;
  /** The SHA-256 of the image the download starts with, for checking by hand: `head -c <bytes> <file> | sha256sum`. */
  expected?: { sha256: string; bytes: number };
  /**
   * For the whole partition table, the MD5 its MD5 entry stores, over the
   * entries before it, as read when the download was planned. The device
   * doesn't hash the file as a whole.
   */
  md5?: string;
}

export type PlanResult =
  | { ok: true; plan: DownloadPlan }
  /** `transient` when asking again later may succeed. */
  | { ok: false; reason: string; transient?: boolean };

/** The size of the stored digest at the end of an image. */
const DIGEST_BYTES = 32;

interface ImageFacts {
  length: number;
  /** Both digests, while the image's check is current. */
  fileSha256?: string;
  digest?: string;
}

/** The image at the start of the region, as far as the device reported it. */
function imageIn(region: Region, flash: InternalFlash): ImageFacts | { missing: string } {
  if (region === "bootloader") {
    const { bootloader } = flash;
    if (bootloader.size === null || bootloader.size === undefined) return { missing: "The bootloader doesn't verify." };
    const facts: ImageFacts = { length: bootloader.size };
    if (bootloader.file_sha256 && bootloader.digest) {
      facts.fileSha256 = bootloader.file_sha256;
      facts.digest = bootloader.digest;
    }
    return facts;
  }
  const app = flash.apps.find((slot) => slot.region === region);
  if (app === undefined) return { missing: "The device doesn't report an image there." };
  const { image } = app;
  if (image.status === "empty") return { missing: "The slot holds no image." };
  if (image.size === null || image.size === undefined) return { missing: "The image's headers don't read." };
  const facts: ImageFacts = { length: image.size };
  if (image.status === "valid" && image.file_sha256 && image.digest) {
    facts.fileSha256 = image.file_sha256;
    facts.digest = image.digest;
  }
  return facts;
}

/** Whether the region starts with an image, so the image alone can be downloaded. */
export function holdsImage(region: Region): boolean {
  return region === "bootloader" || region === "app_current" || region === "app_alternate";
}

/**
 * Plans a download from what the device reported last: the bytes to ask for,
 * the file's name, and what to check them against. Planning happens when the
 * download starts, so a download queued before the device restarted is
 * planned against the boot it runs in.
 */
export function planDownload(
  target: DownloadTarget,
  facts: { flash: InternalFlash; regions: readonly RegionInfo[]; device: string; table?: BoardTable | undefined },
): PlanResult {
  const { flash } = facts;
  const info = facts.regions.find((region) => region.region === target.region);
  if (info === undefined) return { ok: false, reason: "The device no longer lists that region." };
  const locked = lockReason(info, flash.tier);
  if (locked !== undefined) return { ok: false, reason: `${locked}.` };

  const slot = flash.apps.find((app) => app.region === target.region)?.slot;
  const found = holdsImage(target.region) ? imageIn(target.region, flash) : undefined;
  const image = found !== undefined && "length" in found ? found : undefined;

  let span: Span | undefined;
  switch (target.what.kind) {
    case "whole":
      break;
    case "image":
      if (image === undefined) {
        return { ok: false, reason: found !== undefined && "missing" in found ? found.missing : "The region holds no image." };
      }
      if (image.length > info.size) return { ok: false, reason: "The image is larger than its region." };
      span = { first: 0, last: image.length - 1 };
      break;
    case "range": {
      const { first, last } = target.what.span;
      if (first < 0 || last < first || last >= info.size) return { ok: false, reason: "The range is outside the region." };
      span = { first, last };
      break;
    }
  }

  const size = span === undefined ? info.size : span.last - span.first + 1;
  const plan: DownloadPlan = {
    region: target.region,
    size,
    filename: downloadFilename({
      device: facts.device,
      region: target.region,
      slot,
      regionSize: info.size,
      span,
      image: image === undefined ? undefined : { length: image.length, fileSha256: image.fileSha256 },
    }),
  };
  if (slot !== undefined) plan.slot = slot;
  if (span !== undefined) plan.span = span;

  // The image alone hashes to its file_sha256; a whole region starts with the image, whose stored digest covers
  // every byte before the digest. Other ranges aren't checked.
  if (image?.fileSha256 !== undefined) {
    if (span?.first === 0 && size === image.length) {
      plan.expected = { sha256: image.fileSha256, bytes: image.length };
      plan.check = { sha256: image.fileSha256, bytes: image.length, against: "file_sha256" };
    } else if (span === undefined && size >= image.length) {
      plan.expected = { sha256: image.fileSha256, bytes: image.length };
      if (image.digest !== undefined && image.length > DIGEST_BYTES) {
        plan.check = { sha256: image.digest, bytes: image.length - DIGEST_BYTES, against: "digest" };
      }
    }
  }
  if (target.region === "partition_table" && span === undefined) {
    const md5 = tableMd5(info, flash, facts.table);
    if (md5 !== undefined) plan.md5 = md5;
  }
  return { ok: true, plan };
}

/**
 * The MD5 stored in the table the region holds: the board's table when the
 * region is at its offset, else the table at the expected offset when the
 * region is there.
 */
function tableMd5(info: RegionInfo, flash: InternalFlash, table: BoardTable | undefined): string | undefined {
  const board = table?.table;
  if (board !== null && board !== undefined && board.offset === info.offset) return board.md5?.stored;
  if (flash.table.offset === info.offset) return flash.table.md5?.stored;
  return undefined;
}

/** A short name for what is downloaded, such as `app_current image`. */
export function targetLabel(target: DownloadTarget): string {
  switch (target.what.kind) {
    case "whole":
      return target.region === "app_current" || target.region === "app_alternate"
        ? `${target.region}, whole slot`
        : target.region;
    case "image":
      return `${target.region} image`;
    case "range":
      return `${target.region} 0x${target.what.span.first.toString(16)}–0x${target.what.span.last.toString(16)}`;
  }
}
