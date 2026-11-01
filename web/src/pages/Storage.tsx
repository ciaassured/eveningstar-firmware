import { useEffect, useState } from "preact/hooks";
import { describeFailure } from "../api/failure.ts";
import { useServices } from "../app/context.ts";
import { LoadGate } from "../app/LoadGate.tsx";
import { shortStatus } from "../downloads/view.ts";
import { formatBytes, formatExactBytes, formatHex } from "../lib/format.ts";
import { compareEntries, flashSummary, healthNotices, layoutEntries } from "../storage/health.ts";
import type { EntryComparison, FieldName } from "../storage/health.ts";
import {
  roleLabel,
  storageStatusLabel,
  subtypeName,
  tableEndText,
  tableSearchText,
  tierLabel,
  typeName,
  validityLabel,
  otaStateLabel,
} from "../storage/labels.ts";
import { flashTotal, mapSegments, withGaps } from "../storage/map.ts";
import { parseSpan } from "../storage/range.ts";
import type { AppSlot, BoardTable, InternalFlash, Region, RegionInfo } from "../storage/types.ts";
import { bootChain, choiceWhat, firmwareView, imageLength, partitionFacts, regionRow, rowId } from "../storage/view.ts";
import type { RegionRowView } from "../storage/view.ts";
import { Banner } from "../ui/Banner.tsx";
import { Actions, Button } from "../ui/Button.tsx";
import { CopyDigest } from "../ui/Digest.tsx";
import { Dialog } from "../ui/Dialog.tsx";
import { Badge, Badges, Fact, Facts, LockNote, TextButton } from "../ui/Facts.tsx";
import { FlashMap } from "../ui/FlashMap.tsx";
import type { FlashMapSegment } from "../ui/FlashMap.tsx";
import { MenuButton } from "../ui/Menu.tsx";
import { Panel, PanelGrid } from "../ui/Panel.tsx";
import { Progress } from "../ui/Progress.tsx";
import { RegionTable } from "../ui/RegionTable.tsx";
import type { RegionRow } from "../ui/RegionTable.tsx";
import { Status } from "../ui/StatusIcon.tsx";
import { Switch } from "../ui/Switch.tsx";
import pages from "./pages.module.css";
import styles from "./storage.module.css";

const GROUP = "storage";

/** The latest download of a region, beside its button. */
function DownloadState({ region }: { region: Region }) {
  const { downloads } = useServices();
  const status = shortStatus(downloads.latestFor(region));
  return status === undefined ? null : (
    <span class={styles["state"]}>
      <Status level={status.level} label={status.label} />
    </span>
  );
}

/** The download button of a row: a menu for regions that hold an image, or a lock with the reason. */
function DownloadAction({ row, onRange }: { row: RegionRowView; onRange: (region: Region) => void }) {
  const { downloads } = useServices();
  const region = row.region;
  if (region === undefined) return null;
  if (row.lock !== undefined) return <LockNote reason={row.lock} />;
  const choose = (key: string): void => {
    if (key === "range") onRange(region);
    else downloads.add({ region, what: choiceWhat(key === "image" ? "image" : "whole") });
  };
  const [only] = row.choices;
  return (
    <span class={styles["download"]}>
      {row.choices.length === 1 && only !== undefined ? (
        <TextButton label={`Download ${region}, ${only.hint ?? ""}`} onClick={() => choose(only.key)}>
          Download
        </TextButton>
      ) : (
        <MenuButton label="Download" accessibleLabel={`Download from ${region}`} items={row.choices} onSelect={choose} />
      )}
      <DownloadState region={region} />
    </span>
  );
}

const FIELD_LABELS: Record<FieldName, string> = {
  label: "Label",
  type: "Type",
  subtype: "Subtype",
  offset: "Offset",
  size: "Size",
  flags: "Flags",
};

function fieldText(field: FieldName, value: string | number): string {
  if (typeof value === "string") return value;
  switch (field) {
    case "offset":
      return formatHex(value);
    case "size":
      return `${formatBytes(value)} (0x${value.toString(16)})`;
    case "flags":
      return `0x${value.toString(16)}`;
    default:
      return String(value);
  }
}

/** The layout's entry against the board's, field by field. */
function EntryDiff({ comparison }: { comparison: EntryComparison }) {
  const fields: FieldName[] = ["label", "type", "subtype", "offset", "size", "flags"];
  return (
    <table class={styles["mini"]}>
      <caption class="visually-hidden">The layout's entry against the board's table</caption>
      <thead>
        <tr>
          <th scope="col">Field</th>
          <th scope="col">Layout</th>
          <th scope="col">On the board</th>
        </tr>
      </thead>
      <tbody>
        {fields.map((field) => {
          const differs = comparison.differs.includes(field);
          return (
            <tr key={field} class={differs ? styles["differs"] : undefined}>
              <th scope="row">{differs ? <Status level="caution" label={FIELD_LABELS[field]} /> : FIELD_LABELS[field]}</th>
              <td>{comparison.expected === undefined ? "–" : fieldText(field, comparison.expected[field])}</td>
              <td>{comparison.found === undefined ? "Missing" : fieldText(field, comparison.found[field])}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

/** What a row shows when expanded. */
function RegionDetails({
  info,
  row,
  flash,
  comparison,
}: {
  info: RegionInfo;
  row: RegionRowView;
  flash: InternalFlash;
  comparison: EntryComparison | undefined;
}) {
  const { clipboard } = useServices();
  const partition = partitionFacts(info);
  const app = flash.apps.find((slot) => slot.region === info.region);
  const image = info.region === "bootloader" ? flash.bootloader : app?.image;
  const length = imageLength(info.region, flash);
  return (
    <div class={styles["details"]}>
      <Facts compact>
        {partition === undefined ? null : <Fact label="Type">{partition.kind}</Fact>}
        {partition === undefined ? null : (
          <Fact label="Flags" mono>
            {partition.flags}
          </Fact>
        )}
        <Fact label="Offsets" mono>
          {formatHex(info.offset)} to {formatHex(info.offset + info.size - 1)}
        </Fact>
        <Fact label="Size">{`${formatExactBytes(info.size)} (0x${info.size.toString(16)})`}</Fact>
        {info.roles.length === 0 ? null : <Fact label="Roles">{info.roles.map(roleLabel).join(", ")}</Fact>}
        {length === undefined ? null : <Fact label="Image">{formatExactBytes(length)}</Fact>}
        {image?.file_sha256 ? (
          <Fact label="Image SHA-256">
            <CopyDigest value={image.file_sha256} label={`${info.region} image SHA-256`} copy={clipboard} />
          </Fact>
        ) : null}
        {image?.digest ? (
          <Fact label="Validation hash">
            <CopyDigest value={image.digest} label={`${info.region} validation hash`} copy={clipboard} />
          </Fact>
        ) : null}
        {row.lock === undefined ? null : <Fact label="Download">{row.lock}</Fact>}
      </Facts>
      {comparison !== undefined && comparison.differs.length + (comparison.found === undefined ? 1 : 0) > 0 ? (
        <EntryDiff comparison={comparison} />
      ) : null}
    </div>
  );
}

/** The table the bootloader read, against the layout. */
function BoardTableView({ table, regions }: { table: BoardTable; regions: readonly RegionInfo[] }) {
  const read = table.table;
  const offsets = table.offsets.map((offset) => formatHex(offset, 4)).join(" and ");
  if (read === null || read === undefined) {
    return (
      <div class={styles["board"]}>
        <p>{tableSearchText(table.search)}</p>
        {offsets === "" ? null : <p>Tables found at {offsets}.</p>}
      </div>
    );
  }
  const rows = compareEntries(layoutEntries(regions), read.entries);
  const cell = (row: EntryComparison, field: FieldName) => {
    const value = row.found?.[field];
    if (value === undefined) return <td key={field}>–</td>;
    const text = field === "type" ? typeName(value as number) : field === "subtype" ? subtypeName(row.found?.type ?? -1, value as number) : fieldText(field, value);
    return (
      <td key={field} class={row.differs.includes(field) ? styles["differs"] : undefined}>
        {row.differs.includes(field) ? <Status level="caution" label={text} /> : text}
      </td>
    );
  };
  return (
    <div class={styles["board"]}>
      <p>
        The table at {formatHex(read.offset, 4)}. {tableSearchText(table.search)}
      </p>
      <div class={styles["scroll"]}>
        <table class={styles["mini"]}>
          <caption class="visually-hidden">The partition table on the board, against the layout</caption>
          <thead>
            <tr>
              <th scope="col">#</th>
              <th scope="col">Label</th>
              <th scope="col">Type</th>
              <th scope="col">Subtype</th>
              <th scope="col">Offset</th>
              <th scope="col">Size</th>
              <th scope="col">Flags</th>
              <th scope="col">Against the layout</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.index}>
                <td>{row.index}</td>
                {(["label", "type", "subtype", "offset", "size", "flags"] as const).map((field) => cell(row, field))}
                <td>
                  {row.expected === undefined ? (
                    <Status level="caution" label="Extra" />
                  ) : row.found === undefined ? (
                    <Status level="serious" label={`Missing ${row.expected.label}`} />
                  ) : row.differs.length === 0 ? (
                    <Status level="normal" label="Matches" />
                  ) : (
                    <Status level="caution" label={`Differs from ${row.expected.label}`} />
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p class={styles["note"]}>
        {tableEndText(read.end)}
        {read.truncated ? "; it has more entries than are listed." : "."}
      </p>
    </div>
  );
}

function InternalPanel({ flash, regions, table, onRange }: {
  flash: InternalFlash;
  regions: readonly RegionInfo[];
  table: BoardTable | undefined;
  onRange: (region: Region) => void;
}) {
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [view, setView] = useState<"layout" | "board">("layout");
  const areas = withGaps(regions, flashTotal(flash.capacity, flash.layout_size, regions));
  const segments: FlashMapSegment[] = mapSegments(areas, flash.apps).map((segment) => ({
    key: segment.key,
    name: segment.name,
    kind:
      segment.region === undefined
        ? "gap"
        : segment.contents === "sensitive"
          ? "sensitive"
          : segment.contents === "reserved"
            ? "reserved"
            : "public",
    start: segment.start,
    width: segment.width,
    current: segment.current,
    fill: segment.fill,
    labelled: segment.labelled,
  }));
  const differs = !flash.table.matches || flash.table.board_offset !== flash.table.offset;
  const comparisons =
    differs && table?.table ? compareEntries(layoutEntries(regions), table.table.entries) : undefined;
  const partitions = regions.filter((info) => info.partition !== null && info.partition !== undefined);

  const rows: RegionRow[] = areas.map((area) => {
    const row = regionRow(area, flash);
    if (area.kind === "gap") return { ...row };
    const index = partitions.indexOf(area.info);
    return {
      ...row,
      action: <DownloadAction row={row} onRange={onRange} />,
      details: (
        <RegionDetails info={area.info} row={row} flash={flash} comparison={index < 0 ? undefined : comparisons?.[index]} />
      ),
    };
  });
  const toggle = (key: string): void => {
    const next = new Set(expanded);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    setExpanded(next);
  };
  const select = (key: string): void => {
    setView("layout");
    if (!key.startsWith("gap-")) setExpanded(new Set([...expanded, key]));
    const element = document.getElementById(rowId(key));
    element?.scrollIntoView({ block: "center", behavior: "smooth" });
    element?.focus({ preventScroll: true });
  };
  const tier = tierLabel(flash.tier);

  return (
    <Panel title="Internal flash">
      <p class={styles["summary"]}>
        <Status level={tier.level} />
        <span>{flashSummary(flash).join(" · ")}</span>
      </p>
      <FlashMap segments={segments} onSelect={select} />
      <div class={styles["tableHead"]}>
        <h3 class={styles["subheading"]}>Regions</h3>
        {differs ? (
          <Switch
            label="Which table to show"
            value={view}
            options={[
              { key: "layout", label: "Layout" },
              { key: "board", label: "On the board" },
            ]}
            onChange={setView}
          />
        ) : null}
      </div>
      {view === "board" && table !== undefined ? (
        <BoardTableView table={table} regions={regions} />
      ) : (
        <RegionTable caption="Regions of the internal flash" rows={rows} expanded={expanded} onToggle={toggle} />
      )}
    </Panel>
  );
}

function FirmwareCard({ app, flash, regions }: { app: AppSlot; flash: InternalFlash; regions: readonly RegionInfo[] }) {
  const { downloads, clipboard } = useServices();
  const view = firmwareView(app, flash, regions);
  const info = regions.find((region) => region.region === app.region);
  const downloadable = info !== undefined && info.contents === "public" && info.available;
  const { image } = app;
  return (
    <Panel title={view.title}>
      {view.badges.length > 0 ? (
        <Badges>
          {view.badges.map((badge) => (
            <Badge key={badge.label} accent={badge.accent}>
              {badge.label}
            </Badge>
          ))}
        </Badges>
      ) : null}
      {view.fellBack === undefined ? null : (
        <Banner level="caution" title="The bootloader fell back">
          <p>{view.fellBack}</p>
        </Banner>
      )}
      <Facts compact>
        <Fact label="Image">
          <Status level={view.status.level} label={view.fault === undefined ? view.status.label : `${view.status.label}: ${view.fault}`} />
        </Fact>
        {view.name === undefined ? null : <Fact label="Firmware">{view.name}</Fact>}
        {view.built === undefined ? null : <Fact label="Built">{view.built}</Fact>}
        {view.idf === undefined ? null : <Fact label="ESP-IDF or HAL">{view.idf}</Fact>}
        {view.secureVersion === undefined ? null : <Fact label="Secure version">{view.secureVersion}</Fact>}
        {image.file_sha256 ? (
          <Fact label="SHA-256">
            <CopyDigest value={image.file_sha256} label={`${view.title} image SHA-256`} copy={clipboard} />
          </Fact>
        ) : null}
        {image.descriptor ? (
          <Fact label="ELF SHA-256">
            <CopyDigest value={image.descriptor.elf_sha256} label={`${view.title} ELF SHA-256`} copy={clipboard} />
          </Fact>
        ) : null}
        {view.fill === undefined ? null : (
          <Fact label="Size">
            <span class={styles["fill"]}>
              <span>{view.fill.text}</span>
              <Progress value={view.fill.share} label={`${view.title}: image size against the slot`} tone="neutral" />
            </span>
          </Fact>
        )}
      </Facts>
      {downloadable ? (
        <Actions>
          {view.canDownloadImage ? (
            <Button onClick={() => downloads.add({ region: app.region, what: { kind: "image" } })}>Download image</Button>
          ) : null}
          <Button onClick={() => downloads.add({ region: app.region, what: { kind: "whole" } })}>Whole slot</Button>
          <DownloadState region={app.region} />
        </Actions>
      ) : null}
    </Panel>
  );
}

function BootChainPanel({ flash }: { flash: InternalFlash }) {
  const { clipboard } = useServices();
  const lines = bootChain(flash);
  const { bootloader } = flash;
  return (
    <Panel title="Boot chain">
      <Facts>
        {lines.map((line) => (
          <Fact key={line.label} label={line.label}>
            {line.level === undefined ? line.text : <Status level={line.level} label={line.text} />}
          </Fact>
        ))}
        {bootloader.file_sha256 ? (
          <Fact label="SHA-256">
            <CopyDigest value={bootloader.file_sha256} label="bootloader SHA-256" copy={clipboard} />
          </Fact>
        ) : null}
      </Facts>
      {flash.otadata.length > 0 ? (
        <details class={styles["otadata"]}>
          <summary>Otadata entries</summary>
          <div class={styles["scroll"]}>
            <table class={styles["mini"]}>
              <thead>
                <tr>
                  <th scope="col">Sector</th>
                  <th scope="col">Entry</th>
                  <th scope="col">Sequence</th>
                  <th scope="col">State</th>
                  <th scope="col">Selects</th>
                </tr>
              </thead>
              <tbody>
                {flash.otadata.map((entry) => {
                  const validity = validityLabel(entry.validity);
                  return (
                    <tr key={entry.sector}>
                      <td>{entry.sector}</td>
                      <td>
                        <Status level={validity.level} label={validity.label} />
                      </td>
                      <td>{entry.seq ?? "–"}</td>
                      <td>{entry.state === null || entry.state === undefined ? "–" : otaStateLabel(entry.state)}</td>
                      <td class={styles["monoCell"]}>{entry.selects ?? "–"}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </details>
      ) : null}
    </Panel>
  );
}

/** Asks for the bytes of a region to download, as offsets within it. */
function RangeDialog({ region, size, onClose }: { region: Region | undefined; size: number; onClose: () => void }) {
  const { downloads } = useServices();
  const [from, setFrom] = useState("0x0");
  const [to, setTo] = useState("");
  const [error, setError] = useState<string | undefined>(undefined);
  useEffect(() => {
    setFrom("0x0");
    setTo("");
    setError(undefined);
  }, [region]);
  const submit = (): void => {
    if (region === undefined) return;
    const parsed = parseSpan(from, to, size);
    if (!parsed.ok) {
      setError(parsed.error);
      return;
    }
    downloads.add({ region, what: { kind: "range", span: parsed.span } });
    onClose();
  };
  return (
    <Dialog open={region !== undefined} title={`Download part of ${region ?? ""}`} onClose={onClose}>
      <p>
        Offsets within the region, both included, in decimal or with 0x in hexadecimal. The region is{" "}
        {formatExactBytes(size)}, up to 0x{Math.max(0, size - 1).toString(16)}.
      </p>
      <div class={styles["range"]}>
        <label class={styles["field"]}>
          <span>First byte</span>
          <input
            class={styles["input"]}
            value={from}
            inputMode="text"
            spellcheck={false}
            onInput={(event) => {
              setFrom(event.currentTarget.value);
            }}
          />
        </label>
        <label class={styles["field"]}>
          <span>Last byte (empty for the end)</span>
          <input
            class={styles["input"]}
            value={to}
            inputMode="text"
            spellcheck={false}
            onInput={(event) => {
              setTo(event.currentTarget.value);
            }}
          />
        </label>
      </div>
      {error === undefined ? null : (
        <p class={styles["error"]} role="alert">
          {error}
        </p>
      )}
      <Actions>
        <Button variant="primary" onClick={submit}>
          Add to downloads
        </Button>
        <Button onClick={onClose}>Cancel</Button>
      </Actions>
    </Dialog>
  );
}

function Content() {
  const { storage } = useServices();
  const [rangeFor, setRangeFor] = useState<Region | undefined>(undefined);
  useEffect(() => storage.watch(), [storage]);

  const flash = storage.flash.value;
  const regions = storage.regions.value;
  const devices = storage.devices.value;
  if (flash === undefined || regions === undefined) return null;
  const failure = storage.error.value;
  const others = (devices?.devices ?? []).filter((device) => device.id !== "internal");
  const rangeSize = regions.find((info) => info.region === rangeFor)?.size ?? 0;

  return (
    <>
      <div class={styles["head"]}>
        <h1 class={pages["heading"]}>Storage</h1>
        <Button
          disabled={storage.refreshing.value}
          reason={storage.refreshing.value ? "Refreshing…" : undefined}
          onClick={() => {
            void storage.refresh("action");
          }}
        >
          Refresh
        </Button>
      </div>
      {failure === undefined ? null : (
        <Banner level="caution" title={`Can't refresh: ${describeFailure(failure).title}`}>
          <p>{describeFailure(failure).explanation} The values below are from the last time it worked.</p>
        </Banner>
      )}
      {healthNotices(flash).map((notice) => (
        <Banner key={notice.key} level={notice.level} title={notice.title} urgent={notice.level === "serious"}>
          {notice.lines.map((line) => (
            <p key={line}>{line}</p>
          ))}
        </Banner>
      ))}
      <InternalPanel flash={flash} regions={regions} table={storage.table.value} onRange={setRangeFor} />
      <h2 class={styles["section"]}>Firmware</h2>
      <PanelGrid>
        {flash.apps.map((app) => (
          <FirmwareCard key={app.region} app={app} flash={flash} regions={regions} />
        ))}
      </PanelGrid>
      <BootChainPanel flash={flash} />
      {others.map((device) => {
        const status = storageStatusLabel(device.status);
        return (
          <Panel key={device.id} title={device.id === "external" ? "External flash" : `Storage "${device.id}"`}>
            <p class={styles["summary"]}>
              <Status level={status.level} label={status.label} />
              <span>
                {device.status === "not_supported"
                  ? "On the board, not supported by this firmware yet"
                  : device.capacity === null || device.capacity === undefined
                    ? ""
                    : formatBytes(device.capacity)}
              </span>
            </p>
          </Panel>
        );
      })}
      <RangeDialog
        region={rangeFor}
        size={rangeSize}
        onClose={() => {
          setRangeFor(undefined);
        }}
      />
    </>
  );
}

export function Storage() {
  const { storage } = useServices();
  return (
    <LoadGate group={GROUP} loaders={storage.loaders(GROUP)}>
      <Content />
    </LoadGate>
  );
}
