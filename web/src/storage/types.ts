import type { components } from "../api/generated/v1.d.ts";

type Schemas = components["schemas"];

export type StorageDevices = Schemas["StorageDevices"];
export type StorageDevice = Schemas["StorageDevice"];
export type InternalFlash = Schemas["InternalFlash"];
export type Regions = Schemas["Regions"];
export type RegionInfo = Schemas["RegionInfo"];
export type Region = Schemas["Region"];
export type BoardTable = Schemas["BoardTable"];
export type TableEntry = Schemas["TableEntry"];
export type AppSlot = Schemas["AppSlot"];
export type AppImage = Schemas["AppImage"];
export type BootloaderInfo = Schemas["BootloaderInfo"];
export type OtadataEntry = Schemas["OtadataEntry"];
export type PartitionKind = Schemas["PartitionKind"];
export type Tier = Schemas["Tier"];
export type Limit = Schemas["Limit"];
export type Role = Schemas["Role"];
export type ImageStatus = Schemas["ImageStatus"];
export type ImageFault = Schemas["ImageFault"];
export type Contents = Schemas["Contents"];
export type MatchStatus = Schemas["MatchStatus"];
export type EntryField = Schemas["EntryField"];
export type SlotName = Schemas["SlotName"];
export type OtaState = Schemas["OtaState"];
export type EntryValidity = Schemas["EntryValidity"];
export type BootloaderStatus = Schemas["BootloaderStatus"];
export type TableSearch = Schemas["TableSearch"];
export type TableEnd = Schemas["TableEnd"];
export type StorageStatus = Schemas["StorageStatus"];

/** A value the API names today, or one from a newer firmware, shown as it came. */
export type Open<T extends string> = T | (string & {});

/** Inclusive offsets into a region. */
export interface Span {
  first: number;
  last: number;
}
