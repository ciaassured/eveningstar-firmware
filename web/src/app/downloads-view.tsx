import { signal } from "@preact/signals";
import { createRef } from "preact";
import { useEffect, useRef } from "preact/hooks";
import { buttonView, finishedIds, trayCount, trayItems } from "../downloads/view.ts";
import type { ViewFacts } from "../downloads/view.ts";
import { USAGE, helpTopic } from "../storage/copy.ts";
import { planDownload } from "../storage/plan.ts";
import { DownloadTray, DownloadsButton } from "../ui/DownloadTray.tsx";
import type { TrayAction } from "../ui/DownloadTray.tsx";
import { inPopover, popoverOpen } from "../ui/Popover.tsx";
import type { Services } from "./services.ts";
import { useServices } from "./context.ts";

const TRAY_ID = "downloads-tray";

/** Whether the tray is open; the header's button and the tray share it. */
const open = signal(false);
const button = createRef<HTMLButtonElement>();

function close(focus: boolean): void {
  open.value = false;
  if (focus) button.current?.focus();
}

/** What the tray needs of the storage service: the regions, and a plan for each download not started yet. */
function viewFacts({ storage, device, now }: Services): ViewFacts {
  const flash = storage.flash.value;
  const regions = storage.regions.value ?? [];
  const table = storage.table.value;
  const name = device.system.value.value?.name ?? "eveningstar";
  return {
    now: now.value,
    regions,
    preview(target) {
      if (flash === undefined) return undefined;
      const planned = planDownload(target, { flash, regions, device: name, table });
      return planned.ok ? planned.plan : undefined;
    },
  };
}

/** The header's Downloads button. It pulses when a download is added. */
export function DownloadsHeaderButton() {
  const services = useServices();
  const entries = services.downloads.entries.value;
  const view = buttonView(trayItems(entries, viewFacts(services)));
  const newest = entries.reduce((max, entry) => Math.max(max, entry.id), 0);
  const seen = useRef(newest);

  useEffect(() => {
    if (newest > seen.current && !globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      button.current?.animate([{ transform: "scale(1)" }, { transform: "scale(1.08)" }, { transform: "scale(1)" }], 360);
    }
    seen.current = newest;
  }, [newest]);

  return (
    <DownloadsButton
      {...view}
      expanded={open.value}
      controls={TRAY_ID}
      buttonRef={button}
      onClick={() => {
        open.value = !open.peek();
      }}
    />
  );
}

/**
 * The downloads tray. It closes on Escape, wherever focus is, unless a
 * popover takes it; on a pointer press outside it, a popover and the button;
 * and on its close button.
 */
export function DownloadsTray({ class: className }: { class?: string | undefined }) {
  const services = useServices();
  const { downloads, clipboard } = services;
  const tray = useRef<HTMLElement>(null);
  const items = trayItems(downloads.entries.value, viewFacts(services));
  const shown = open.value;

  useEffect(() => {
    if (!shown) return;
    const key = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || event.defaultPrevented || popoverOpen()) return;
      // The page's own Escape, such as closing the phone's inspector, waits for the next press.
      event.preventDefault();
      close(true);
    };
    const press = (event: PointerEvent): void => {
      const { target } = event;
      if (!(target instanceof Node) || tray.current?.contains(target) === true || button.current?.contains(target) === true) return;
      if (!inPopover(target)) close(false);
    };
    document.addEventListener("keydown", key, true);
    document.addEventListener("pointerdown", press, true);
    return () => {
      document.removeEventListener("keydown", key, true);
      document.removeEventListener("pointerdown", press, true);
    };
  }, [shown]);

  const act = (id: number, action: TrayAction): void => {
    downloads.act(id, action === "remove" ? "dismiss" : action);
  };

  return (
    <DownloadTray
      id={TRAY_ID}
      class={className}
      open={shown}
      items={items}
      count={trayCount(items)}
      trayRef={tray}
      onClose={() => {
        close(true);
      }}
      onClear={() => {
        for (const id of finishedIds(items)) downloads.act(id, "dismiss");
      }}
      onAction={act}
      copy={clipboard}
      spanHelp={{ label: USAGE.endHelp, ...helpTopic("span", 0) }}
    />
  );
}
