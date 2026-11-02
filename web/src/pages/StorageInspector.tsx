import { DownloadActions, DownloadButton, NoteBox } from "../ui/DownloadButton.tsx";
import { Fact, Facts, RoleBoxes } from "../ui/Facts.tsx";
import { HashRow, Hashes } from "../ui/Hashes.tsx";
import { StatusPill } from "../ui/Pill.tsx";
import { Placement } from "../ui/Placement.tsx";
import { HelpButton } from "../ui/Popover.tsx";
import { regionClass } from "../ui/region.ts";
import { Ruler } from "../ui/Ruler.tsx";
import { Section } from "../ui/Section.tsx";
import { SpanTable } from "../ui/SpanTable.tsx";
import type { SpanHeaders } from "../ui/SpanTable.tsx";
import { StatusLine, StatusMark } from "../ui/StatusMark.tsx";
import { Swatch } from "../ui/Swatch.tsx";
import { Table } from "../ui/Table.tsx";
import { ACTIONS, DETAILS, PAGE, USAGE, helpTopic } from "../storage/copy.ts";
import type {
  Action,
  Cell,
  CompareValue,
  DetailsView,
  EntryPanel,
  ExpectedView,
  InspectorView,
  OverlapView,
  Run,
  SpanRow,
  UsageView,
} from "../storage/inspector.ts";
import { cx } from "../lib/cx.ts";
import styles from "./storage.module.css";

/** Writes text to the clipboard; `undefined` where the page has none. */
type Copy = ((text: string) => Promise<void>) | undefined;

/** The headers of a span table, with the help of "End, exclusive". */
export function spanHeaders(flashSize: number): SpanHeaders {
  return { ...USAGE.headers, help: { label: USAGE.endHelp, ...helpTopic("span", flashSize) } };
}

/** A span table's rows, keyed for the emphasis of parts; a row of another region emphasises that region. */
function spanRows(id: string, rows: readonly SpanRow[]) {
  return rows.map((row) => ({
    part: `${id}:${row.key}`,
    label: row.label,
    square: row.square,
    colour: row.other?.colour,
    region: row.other?.id,
    span: row.span,
  }));
}

/** The usage summary panel: the ruler, its ticks and the span table. */
function Usage({ id, usage, headers }: { id: string; usage: UsageView; headers: SpanHeaders }) {
  return (
    <Section title={usage.title}>
      <Ruler
        part={`${id}:region`}
        texture={usage.texture}
        parts={usage.parts.map((part) => ({
          part: `${id}:${part.key}`,
          fill: part.fill,
          start: part.start,
          width: part.width,
          caption: part.labelled ? part.caption : undefined,
          tooltip: part.tooltip,
        }))}
        text={usage.text}
        ticks={usage.ticks}
      />
      <SpanTable label={usage.tableLabel} headers={headers} rows={spanRows(id, usage.rows)} />
    </Section>
  );
}

function Runs({ runs }: { runs: readonly Run[] }) {
  return (
    <>
      {runs.map((run, i) =>
        run.tone === "plain" ? (
          run.text
        ) : run.tone === "strong" ? (
          <b key={i}>{run.text}</b>
        ) : (
          <span key={i} class={styles[run.tone]}>
            {run.text}
          </span>
        ),
      )}
    </>
  );
}

/** A fact; one with a status icon has the first run beside the icon, in the level's colour. */
function CellFact({ cell }: { cell: Cell }) {
  const level = cell.icon;
  if (level === undefined) {
    return (
      <Fact label={cell.label}>
        <Runs runs={cell.runs} />
      </Fact>
    );
  }
  const [first, ...rest] = cell.runs;
  return (
    <Fact label={cell.label} level={level} marked={first?.text}>
      <Runs runs={rest} />
    </Fact>
  );
}

function Cells({ cells }: { cells: readonly Cell[] }) {
  return cells.length === 0 ? null : (
    <Facts>
      {cells.map((cell) => (
        <CellFact key={cell.label} cell={cell} />
      ))}
    </Facts>
  );
}

/** The details panel: the role boxes, the facts, their notes, the otadata's records and the hashes. */
function Details({ id, details, flashSize, copy }: { id: string; details: DetailsView; flashSize: number; copy: Copy }) {
  const { otadata, hashes } = details;
  return (
    <Section title={details.title} stack>
      {details.roleBoxes === undefined ? null : <RoleBoxes boxes={details.roleBoxes} />}
      <Cells cells={details.cells} />
      {details.notes.map((note) => (
        <p key={note.text} class={note.tone === "note" ? styles["note"] : styles["muted"]}>
          {note.text}
        </p>
      ))}
      {otadata === undefined ? null : (
        <>
          <Table
            kind="records"
            columns={otadata.columns}
            rows={otadata.rows.map((row) => ({
              key: row.sector,
              tone: row.boots ? "strong" : undefined,
              cells: [
                row.sector,
                row.crc.match ? <StatusMark level="ok" word={row.crc.text} /> : <span class={styles["muted"]}>{row.crc.text}</span>,
                row.seq,
                row.state,
                <>
                  {row.selects}
                  {row.boots ? <span class={styles["muted"]}>{DETAILS.nextBoot}</span> : null}
                </>,
              ],
            }))}
          />
          <div class={styles["states"]}>
            {otadata.states.map((state) => (
              <p key={state.state}>
                <b>{state.state}</b>: {state.text}
              </p>
            ))}
            <p class={styles["muted"]}>{otadata.rule}</p>
          </div>
        </>
      )}
      {hashes === undefined ? null : (
        <Hashes title={hashes.title} note={hashes.note} lead={hashes.lead}>
          {hashes.rows.length === 0
            ? undefined
            : hashes.rows.map((row) => (
                <HashRow
                  key={`${id}:${row.key}`}
                  label={row.label}
                  under={row.under}
                  value={row.value}
                  name={row.name}
                  help={row.help === undefined ? undefined : { label: row.helpLabel ?? "", ...helpTopic(row.help, flashSize) }}
                  copy={copy}
                />
              ))}
        </Hashes>
      )}
    </Section>
  );
}

function Value({ value }: { value: CompareValue }) {
  return (
    <>
      <span class={value.mono ? styles["mono"] : undefined}>{value.main}</span>
      {value.sub === undefined ? null : <span class={styles["sub"]}>{value.sub}</span>}
    </>
  );
}

/** The expected state panel: the status line, and for a differing entry the comparison and the placement bars. */
function Expected({ expected, flashSize }: { expected: ExpectedView; flashSize: number }) {
  const { status, comparison, placement } = expected;
  return (
    <Section title={expected.title} after={<HelpButton label={expected.help} {...helpTopic("expected", flashSize)} />}>
      <StatusLine level={status.level}>
        <b>{status.word}.</b> {status.reason}
      </StatusLine>
      {comparison === undefined ? null : (
        <Table
          kind="fields"
          columns={comparison.columns}
          rows={comparison.rows.map((row) => ({
            key: row.field,
            tone: row.differs ? "warn" : undefined,
            cells: [row.field, <Value value={row.expected} />, <Value value={row.board} />],
          }))}
        />
      )}
      {placement === undefined ? null : <Placement {...placement} />}
    </Section>
  );
}

/** The panel of a missing region or an extra entry: the entry's facts and its span. */
function Entry({ id, panel, headers }: { id: string; panel: EntryPanel; headers: SpanHeaders }) {
  return (
    <Section title={panel.title}>
      <Cells cells={panel.cells} />
      <SpanTable label={panel.tableLabel} headers={headers} rows={spanRows(id, panel.rows)} />
    </Section>
  );
}

/** The overlap panel: a line per other region, then the shared ranges. */
function Overlap({ id, overlap, headers }: { id: string; overlap: OverlapView; headers: SpanHeaders }) {
  return (
    <Section title={overlap.title}>
      <div class={styles["lines"]}>
        {overlap.lines.map((line) => (
          <StatusLine key={line} level="warn">
            {line}
          </StatusLine>
        ))}
      </div>
      <SpanTable label={overlap.tableLabel} headers={headers} rows={spanRows(id, overlap.rows)} />
    </Section>
  );
}

/**
 * The region inspector: the header with the status pill, the explanation and
 * the boot line, the downloads or the note in their place, the usage summary,
 * the expected state and the details, the panel of a missing or extra entry,
 * and the overlaps.
 */
export function Inspector({
  view,
  flashSize,
  added,
  onDownload,
  copy,
}: {
  view: InspectorView;
  flashSize: number;
  /** The key of the download button clicked last, while it reads "Added to downloads". */
  added: string | undefined;
  onDownload: (action: Action, key: string) => void;
  copy: Copy;
}) {
  const headers = spanHeaders(flashSize);
  const expected = view.expected === undefined ? null : <Expected expected={view.expected} flashSize={flashSize} />;
  const details = view.details === undefined ? null : <Details id={view.id} details={view.details} flashSize={flashSize} copy={copy} />;
  return (
    <article class={cx(styles["inspector"], regionClass(view.colour))} aria-label={view.label}>
      <header class={styles["head"]} data-region={view.id}>
        <Swatch code={view.swatch} colour={view.colour} texture={view.texture} size="lg" />
        <h2 class={styles["heading"]}>
          {view.title} <span class={styles["subtitle"]}>{view.subtitle}</span>
        </h2>
        <StatusPill level={view.status.level} word={view.pill.word} name={view.pill.name} why={view.why} />
      </header>
      <div class={styles["intro"]}>
        <p class={styles["explain"]}>{view.explanation}</p>
        {view.bootLine === undefined ? null : <p class={styles["bootline"]}>{view.bootLine}</p>}
      </div>
      {view.actions.length > 0 ? (
        <DownloadActions label={PAGE.downloads}>
          {view.actions.map((action) => {
            const key = `${view.id}:${action.key}`;
            return (
              <DownloadButton
                key={key}
                title={action.title}
                sublabel={action.sublabel}
                primary={action.primary}
                disabled={action.disabled}
                added={added === key}
                addedText={ACTIONS.added}
                onClick={() => {
                  onDownload(action, key);
                }}
              />
            );
          })}
        </DownloadActions>
      ) : view.note === undefined ? null : (
        <NoteBox icon={view.note.icon}>{view.note.text}</NoteBox>
      )}
      {view.usage === undefined ? null : <Usage id={view.id} usage={view.usage} headers={headers} />}
      {expected !== null && details !== null ? (
        <div class={styles["cols"]}>
          {expected}
          {details}
        </div>
      ) : (
        (expected ?? details)
      )}
      {view.entryPanel === undefined ? null : <Entry id={view.id} panel={view.entryPanel} headers={headers} />}
      {view.overlap === undefined ? null : <Overlap id={view.id} overlap={view.overlap} headers={headers} />}
    </article>
  );
}
