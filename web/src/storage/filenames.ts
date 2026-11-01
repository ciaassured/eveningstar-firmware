import type { Open, Region, SlotName, Span } from "./types.ts";

/** What a download holds, for naming its file. */
export interface FileSubject {
  /** The device's name on networks, such as `eveningstar-0kd7-9c6`. */
  device: string;
  region: Open<Region>;
  /** The app slot, when the region is one and the slot is known. */
  slot?: Open<SlotName> | null | undefined;
  /** The size of the region. */
  regionSize: number;
  /** The part of the region downloaded; the whole region when `undefined`. */
  span?: Span | undefined;
  /** The image at the start of the region, and the SHA-256 of the whole image while its check is current. */
  image?: { length: number; fileSha256?: string | null | undefined } | undefined;
}

const APP_REGIONS: readonly string[] = ["app_current", "app_alternate"];

/** A name safe in a file name and a shell without quoting. */
function safe(text: string): string {
  return text.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^[-.]+|-+$/g, "") || "device";
}

function hex(value: number): string {
  return `0x${value.toString(16)}`;
}

/**
 * The name the device gives the download in `Content-Disposition`:
 * `<device>-<region>[-<slot>]-<what>.bin`. The image alone is `image-<hash>`
 * and a whole slot holding it `slot-of-<hash>`, the hash being the first 8
 * digits of the image's `file_sha256`; any other part carries its bounds, and
 * a whole region other than an app slot nothing.
 */
export function downloadFilename(subject: FileSubject): string {
  const parts = [safe(subject.device), subject.region];
  if (APP_REGIONS.includes(subject.region) && subject.slot !== null && subject.slot !== undefined) {
    parts.push(subject.slot);
  }
  const span = subject.span ?? { first: 0, last: subject.regionSize - 1 };
  const whole = span.first === 0 && span.last === subject.regionSize - 1;
  const hash = subject.image?.fileSha256?.slice(0, 8);
  const isApp = APP_REGIONS.includes(subject.region);
  if (hash !== undefined && span.first === 0 && span.last === (subject.image?.length ?? 0) - 1) {
    parts.push(`image-${hash}`);
  } else if (whole && isApp && hash !== undefined) {
    parts.push(`slot-of-${hash}`);
  } else if (!whole || isApp) {
    parts.push(`${hex(span.first)}-${hex(span.last)}`);
  }
  return `${parts.map(safe).join("-")}.bin`;
}

/** The name a download is saved under when its bytes didn't match the digest they were checked against. */
export function unverifiedFilename(filename: string): string {
  return filename.replace(/\.bin$/, "") + "-UNVERIFIED.bin";
}
