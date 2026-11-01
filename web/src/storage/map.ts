import type { AppSlot, Contents, Region, RegionInfo } from "./types.ts";

/** A region, or the unallocated space between two. */
export type Area =
  | { kind: "region"; info: RegionInfo; offset: number; size: number }
  | { kind: "gap"; offset: number; size: number };

/**
 * The regions in offset order with the space between them, and after the last
 * one up to `total`, as gaps. Regions that overlap leave no gap.
 */
export function withGaps(regions: readonly RegionInfo[], total: number): Area[] {
  const areas: Area[] = [];
  let cursor = 0;
  for (const info of [...regions].sort((a, b) => a.offset - b.offset)) {
    if (info.offset > cursor) areas.push({ kind: "gap", offset: cursor, size: info.offset - cursor });
    areas.push({ kind: "region", info, offset: info.offset, size: info.size });
    cursor = Math.max(cursor, info.offset + info.size);
  }
  if (total > cursor) areas.push({ kind: "gap", offset: cursor, size: total - cursor });
  return areas;
}

/**
 * Shares of a bar for items of the given sizes: proportional, except that
 * none is narrower than `minShare`, so a small item stays visible. The shares
 * add up to 1.
 */
export function barShares(sizes: readonly number[], minShare: number): number[] {
  const n = sizes.length;
  if (n === 0) return [];
  if (n * minShare >= 1) return sizes.map(() => 1 / n);
  const floored = new Set<number>();
  for (;;) {
    const free = 1 - floored.size * minShare;
    const rest = sizes.reduce((sum, size, i) => (floored.has(i) ? sum : sum + Math.max(0, size)), 0);
    const shares = sizes.map((size, i) =>
      floored.has(i) ? minShare : rest === 0 ? free / (n - floored.size) : (free * Math.max(0, size)) / rest,
    );
    const under = shares.findIndex((share, i) => !floored.has(i) && share < minShare);
    if (under < 0) return shares;
    floored.add(under);
  }
}

/** The narrowest share of the bar an area gets. */
export const MIN_SHARE = 0.012;
/** Areas at least this wide carry their name in the bar; narrower ones are named in a legend. */
export const LABEL_SHARE = 0.09;

/** An area as the flash map draws it. */
export interface MapSegment {
  /** Unique within the map: the region's name, or `gap-<offset>`. */
  key: string;
  region?: Region;
  name: string;
  offset: number;
  size: number;
  contents?: Contents | (string & {});
  /** Where it starts and how wide it is, as shares of the bar. */
  start: number;
  width: number;
  /** Whether it is the running app, which carries the selection bar. */
  current: boolean;
  /** For an app slot with an image, the share of the slot the image fills. */
  fill?: number;
  /** Whether it is wide enough to carry its name. */
  labelled: boolean;
}

/** The name an area goes by on the map and in the table. */
export function areaName(area: Area): string {
  if (area.kind === "gap") return "unallocated";
  return area.info.label ?? (area.info.region === "partition_table" ? "table" : area.info.region);
}

/** The flash map's segments for the areas, with the image each app slot holds. */
export function mapSegments(areas: readonly Area[], apps: readonly AppSlot[]): MapSegment[] {
  const shares = barShares(
    areas.map((area) => area.size),
    MIN_SHARE,
  );
  let start = 0;
  return areas.map((area, i) => {
    const width = shares[i] ?? 0;
    const segment: MapSegment = {
      key: area.kind === "gap" ? `gap-${area.offset}` : area.info.region,
      name: areaName(area),
      offset: area.offset,
      size: area.size,
      start,
      width,
      current: area.kind === "region" && area.info.roles.includes("running"),
      labelled: width >= LABEL_SHARE,
    };
    start += width;
    if (area.kind === "region") {
      segment.region = area.info.region;
      segment.contents = area.info.contents;
      const app = apps.find((slot) => slot.region === area.info.region);
      const imageSize = app?.image.size;
      if (app !== undefined && imageSize !== null && imageSize !== undefined && app.size > 0) {
        segment.fill = Math.min(1, imageSize / app.size);
      }
    }
    return segment;
  });
}

/** The size of the flash the map spans: the chip's, or the layout's when the chip's isn't known. */
export function flashTotal(capacity: number, layoutSize: number, regions: readonly RegionInfo[]): number {
  const end = regions.reduce((max, info) => Math.max(max, info.offset + info.size), 0);
  return Math.max(capacity > 0 ? capacity : layoutSize, end);
}
