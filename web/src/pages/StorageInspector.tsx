import { DownloadActions, DownloadButton, NoteBox } from "../ui/DownloadButton.tsx";
import { StatusPill } from "../ui/Pill.tsx";
import { regionClass } from "../ui/region.ts";
import { Ruler } from "../ui/Ruler.tsx";
import { Section } from "../ui/Section.tsx";
import { SpanTable } from "../ui/SpanTable.tsx";
import type { SpanHeaders } from "../ui/SpanTable.tsx";
import { Swatch } from "../ui/Swatch.tsx";
import { ACTIONS, PAGE, USAGE, helpTopic } from "../storage/copy.ts";
import type { Action, InspectorView, UsageView } from "../storage/inspector.ts";
import { cx } from "../lib/cx.ts";
import styles from "./storage.module.css";

/** The headers of a span table, with the help of "End, exclusive". */
export function spanHeaders(flashSize: number): SpanHeaders {
  return { ...USAGE.headers, help: { label: USAGE.endHelp, ...helpTopic("span", flashSize) } };
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
      <SpanTable
        label={usage.tableLabel}
        headers={headers}
        rows={usage.rows.map((row) => ({ part: `${id}:${row.key}`, label: row.label, square: row.square, span: row.span }))}
      />
    </Section>
  );
}

/**
 * The region inspector: the header with the status pill, the explanation and
 * the boot line, the downloads or the note in their place, and the usage
 * summary.
 */
export function Inspector({
  view,
  flashSize,
  added,
  onDownload,
}: {
  view: InspectorView;
  flashSize: number;
  /** The key of the download button clicked last, while it reads "Added to downloads". */
  added: string | undefined;
  onDownload: (action: Action, key: string) => void;
}) {
  const headers = spanHeaders(flashSize);
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
    </article>
  );
}

