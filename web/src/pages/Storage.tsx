import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "preact/hooks";
import { describeFailure } from "../api/failure.ts";
import { useServices } from "../app/context.ts";
import { LoadGate } from "../app/LoadGate.tsx";
import { formatClock, formatSize } from "../lib/format.ts";
import { PAGE, STRIP, helpTopic } from "../storage/copy.ts";
import { inspectorView } from "../storage/inspector.ts";
import type { Action } from "../storage/inspector.ts";
import { buildModel, keepSelection } from "../storage/items.ts";
import type { Item, StorageModel } from "../storage/items.ts";
import { createStoragePage } from "../storage/page.ts";
import type { StoragePage } from "../storage/page.ts";
import { readout, readoutEnds, usagePart } from "../storage/strip.ts";
import { summaryView } from "../storage/summary.ts";
import { Button } from "../ui/Button.tsx";
import { useEmphasis } from "../ui/emphasis.ts";
import { useMedia } from "../ui/media.ts";
import { PopoverHost, closePopover } from "../ui/Popover.tsx";
import { RegionList } from "../ui/RegionList.tsx";
import type { RegionRow } from "../ui/RegionList.tsx";
import { SplitPane } from "../ui/SplitPane.tsx";
import { Strip } from "../ui/Strip.tsx";
import type { StripBlock } from "../ui/Strip.tsx";
import { Inspector } from "./StorageInspector.tsx";
import styles from "./storage.module.css";

const GROUP = "storage";
const PHONE = "(max-width: 759px)";

function stripBlock(item: Item): StripBlock {
  const part = item.usage === undefined || item.info === undefined ? undefined : usagePart(item.info.region);
  return {
    id: item.id,
    code: item.code,
    title: item.title,
    colour: item.colour,
    texture: item.texture,
    lock: item.lock,
    warn: item.status.level !== "ok",
    grow: item.grow,
    usage: item.usage,
    usagePart: part === undefined ? undefined : `${item.id}:${part}`,
    name: item.name,
  };
}

function listRow(item: Item): RegionRow {
  return {
    id: item.id,
    code: item.swatch,
    colour: item.colour,
    texture: item.texture,
    title: item.title,
    subtitle: item.subtitle,
    missing: item.kind === "missing",
    size: formatSize(item.span.size),
    roles: item.roles.map((chip) => ({
      role: chip.role,
      label: chip.label,
      variant: chip.role === "selected" ? chip.variant : undefined,
      tooltip: chip.role === "selected" ? chip.tooltip : undefined,
    })),
    status: item.status,
  };
}

/** The bar at the top: the page's title, and the Refresh button with why the last reload failed. */
function TitleBar() {
  const { storage } = useServices();
  const failure = storage.error.value;
  const updated = storage.updatedAt.value;
  const described = failure === undefined ? undefined : describeFailure(failure);
  const refreshing = storage.refreshing.value;
  return (
    <div class={styles["titlebar"]}>
      <h1 class={styles["title"]}>{PAGE.title}</h1>
      <div class={styles["refresh"]}>
        {described === undefined ? null : (
          <span class={styles["failed"]} title={described.explanation}>
            {PAGE.notRefreshed(described.title, updated === undefined ? "–" : formatClock(updated))}
          </span>
        )}
        <Button
          disabled={refreshing}
          reason={refreshing ? PAGE.refreshing : undefined}
          onClick={() => {
            void storage.refresh("action");
          }}
        >
          {PAGE.refresh}
        </Button>
      </div>
    </div>
  );
}

/** The DOM work a selection or a step back leaves for after the page has drawn it. */
interface After {
  /** Where focus goes: the phone's Back button, or the selected row. */
  focus?: "back" | "row";
  /** Whether the inspector scrolls to its top and the row into view. */
  reveal?: boolean;
}

function Frame({ model, page }: { model: StorageModel; page: StoragePage }) {
  const { downloads } = useServices();
  const phone = useMedia(PHONE);
  const root = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const detailRef = useRef<HTMLDivElement>(null);
  const backRef = useRef<HTMLButtonElement>(null);
  const after = useRef<After>({});
  const [step, setStep] = useState(0);
  useEmphasis(root);

  const selected = keepSelection(model, page.selected.value);
  const slide = page.slide.value;
  useEffect(() => {
    page.sync(model);
  }, [model]);

  const rowOf = (id: string | undefined): HTMLElement | null | undefined =>
    id === undefined ? undefined : listRef.current?.querySelector<HTMLElement>(`[data-region="${CSS.escape(id)}"]`);

  useLayoutEffect(() => {
    const { focus, reveal } = after.current;
    after.current = {};
    const row = rowOf(page.selected.peek());
    if (reveal === true) {
      if (detailRef.current !== null) detailRef.current.scrollTop = 0;
      if (!phone) row?.scrollIntoView({ block: "nearest" });
    }
    if (focus === "back") backRef.current?.focus({ preventScroll: true });
    else if (focus === "row") row?.focus({ preventScroll: true });
  }, [step]);

  const select = (id: string, keyboard: boolean): void => {
    page.select(id, phone);
    after.current = { reveal: true, ...(phone ? { focus: "back" } : keyboard ? { focus: "row" } : {}) };
    setStep((n) => n + 1);
  };
  const back = (): void => {
    closePopover();
    page.back();
    after.current = { focus: "row" };
    setStep((n) => n + 1);
  };

  // Escape closes the phone's inspector once no popover took it.
  useEffect(() => {
    const key = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || event.defaultPrevented || !phone || page.slide.peek() === "closed") return;
      back();
    };
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("keydown", key);
    };
  }, [phone]);

  const view = selected === undefined ? undefined : inspectorView(model, selected);
  const summary = summaryView(model);
  return (
    <div ref={root} class={styles["frame"]}>
      <TitleBar />
      <Strip
        label={STRIP.label(model.flashSize)}
        blocks={model.items.map(stripBlock)}
        selected={selected}
        onSelect={select}
        readouts={model.items.map((item) => ({ id: item.id, ...readout(item) }))}
        ends={readoutEnds(model.stripEnd)}
        help={{ label: STRIP.help, ...helpTopic("strip", model.flashSize) }}
      />
      <SplitPane
        phone={phone}
        shown={slide === "open"}
        open={slide !== "closed"}
        detailRef={detailRef}
        backRef={backRef}
        back={{ label: PAGE.back, crumb: PAGE.crumb, onBack: back }}
        master={
          <RegionList
            label={PAGE.list}
            summary={summary}
            rows={model.items.map(listRow)}
            selected={selected}
            onSelect={select}
            listRef={listRef}
          />
        }
        detail={
          view === undefined ? null : (
            <Inspector
              view={view}
              flashSize={model.flashSize}
              added={page.added.value}
              onDownload={(action: Action, key: string) => {
                downloads.add(action.target);
                page.markAdded(key);
              }}
            />
          )
        }
      />
      <PopoverHost />
    </div>
  );
}

function Content() {
  const { storage, timers } = useServices();
  useEffect(() => storage.watch(), [storage]);
  const page = useMemo(
    () =>
      createStoragePage({
        timers,
        frame: (callback) => {
          globalThis.requestAnimationFrame(() => {
            callback();
          });
        },
      }),
    [],
  );
  useEffect(
    () => () => {
      page.dispose();
      closePopover();
    },
    [page],
  );

  const flash = storage.flash.value;
  const regions = storage.regions.value;
  const table = storage.table.value;
  const devices = storage.devices.value;
  const model = useMemo(
    () => (flash === undefined || regions === undefined ? undefined : buildModel({ flash, regions, table, devices })),
    [flash, regions, table, devices],
  );
  if (model === undefined) return null;
  return <Frame model={model} page={page} />;
}

export function Storage() {
  const { storage } = useServices();
  return (
    <div class={styles["page"]}>
      <LoadGate group={GROUP} loaders={storage.loaders(GROUP)}>
        <Content />
      </LoadGate>
    </div>
  );
}
