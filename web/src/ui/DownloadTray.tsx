import type { Ref } from "preact";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { cx } from "../lib/cx.ts";
import type { SpanText } from "../lib/format.ts";
import { TOP_SLACK, keepPlace } from "../lib/place.ts";
import styles from "./downloadtray.module.css";
import { HashRow } from "./Hashes.tsx";
import { Icon } from "./Icon.tsx";
import type { IconName } from "./Icon.tsx";
import { HelpButton } from "./Popover.tsx";
import { Swatch } from "./Swatch.tsx";
import { percent, useVars } from "./vars.ts";

/** The five statuses of a download in the tray. */
export type DownloadStatus = "pending" | "downloading" | "completed" | "failed" | "cancelled";

/** What can be done with a download in the tray. */
export type TrayAction = "retry" | "cancel" | "remove" | "save-anyway";

/** A download as the tray shows it, every part already in words. */
export interface TrayItem {
  id: number;
  status: DownloadStatus;
  word: string;
  amount: string;
  /** The share done, while downloading with a known size. */
  share?: number;
  filename: string;
  what: string;
  /** Why it failed. */
  reason?: string;
  /** When it was queued: the clock and the age. */
  queued: string;
  from: { code: string; colour: string; title: string; subtitle: string };
  /** The "Progress" fact. */
  progress: string;
  /** The range downloaded, once the region is known. */
  span?: SpanText;
  hash?: { label: string; text: string; value: string };
  notes: readonly string[];
  actions: readonly TrayAction[];
}

const ICONS: Record<DownloadStatus, IconName> = {
  pending: "pending",
  downloading: "download",
  completed: "ok",
  failed: "failed",
  cancelled: "cancelled",
};

const ACTIONS: Record<TrayAction, { label: string; icon?: IconName }> = {
  retry: { label: "Retry", icon: "rollback" },
  cancel: { label: "Cancel", icon: "cancelled" },
  "save-anyway": { label: "Save anyway" },
  remove: { label: "Remove" },
};

/**
 * The header's Downloads button: a ring with the running download's share
 * around the download icon, the label, and a badge with how many downloads
 * are active, hidden when none is.
 */
export function DownloadsButton({
  active,
  share,
  label,
  expanded,
  controls,
  buttonRef,
  onClick,
}: {
  active: number;
  share: number;
  /** The accessible name, with the figures. */
  label: string;
  expanded: boolean;
  controls: string;
  buttonRef: Ref<HTMLButtonElement>;
  onClick: () => void;
}) {
  const ring = useVars<HTMLSpanElement>({ "--p": String(share) });
  return (
    <button
      ref={buttonRef}
      type="button"
      class={styles["button"]}
      aria-label={label}
      aria-expanded={expanded}
      aria-controls={controls}
      onClick={onClick}
    >
      <span ref={ring} class={styles["ring"]}>
        <svg class={styles["arc"]} viewBox="0 0 22 22" aria-hidden="true" focusable="false">
          <circle class={styles["track"]} cx="11" cy="11" r="9" />
          <circle class={styles["fill"]} cx="11" cy="11" r="9" pathLength={100} />
        </svg>
        <svg class={styles["down"]} viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path d="M8 2.5v8M4.5 7.5 8 11l3.5-3.5M3 13.5h10" />
        </svg>
      </span>
      <span class={styles["label"]}>Downloads</span>
      {active > 0 ? <span class={styles["badge"]}>{active}</span> : null}
    </button>
  );
}

function Bar({ share }: { share: number }) {
  const ref = useVars<HTMLSpanElement>({ "--p": percent(Math.min(1, share)) });
  return (
    <span class={styles["bar"]}>
      <span ref={ref} />
    </span>
  );
}

/** A range as the tray shows it: its start, end (exclusive) and size, in hex over decimal. */
function SpanValue({ span, help }: { span: SpanText; help: { label: string; title: string; text: string } }) {
  return (
    <div class={styles["span"]}>
      <div>
        <span class={styles["key"]}>Start</span>
        <span class={styles["hex"]}>{span.start.hex}</span>
        <span class={styles["dec"]}>{span.start.decimal}</span>
      </div>
      <div>
        <span class={styles["key"]}>
          End, exclusive
          <HelpButton {...help} />
        </span>
        <span class={styles["hex"]}>{span.end.hex}</span>
        <span class={styles["dec"]}>{span.end.decimal}</span>
      </div>
      <div>
        <span class={styles["key"]}>Size</span>
        <span class={styles["hex"]}>{span.size.hex}</span>
        <span class={styles["dec"]}>{span.size.text}</span>
      </div>
    </div>
  );
}

interface ItemProps {
  item: TrayItem;
  open: boolean;
  onToggle: () => void;
  onAction: (action: TrayAction) => void;
  copy: ((text: string) => Promise<void>) | undefined;
  spanHelp: { label: string; title: string; text: string };
}

function Item({ item, open, onToggle, onAction, copy, spanHelp }: ItemProps) {
  return (
    <li class={cx(styles["item"], styles[item.status], open && styles["open"])} data-id={item.id}>
      <button type="button" class={styles["head"]} aria-expanded={open} onClick={onToggle}>
        <span class={styles["icon"]}>
          <Icon name={ICONS[item.status]} size={18} />
        </span>
        <span class={styles["file"]}>{item.filename}</span>
        <span class={styles["state"]}>
          <span class={styles["word"]}>{item.word}</span>
          <span class={styles["amount"]}>{item.amount}</span>
        </span>
        <span class={styles["what"]}>{item.what}</span>
        {item.status === "downloading" ? <Bar share={item.share ?? 0} /> : null}
      </button>
      {open ? (
        <div class={styles["body"]}>
          {item.reason === undefined ? null : <p class={styles["reason"]}>{item.reason}</p>}
          <dl class={styles["facts"]}>
            <dt>Queued</dt>
            <dd>{item.queued}</dd>
            <dt>From</dt>
            <dd class={styles["from"]}>
              <Swatch code={item.from.code} colour={item.from.colour} texture="public" size="sm" />
              {item.from.title}
              <span class={styles["muted"]}>{item.from.subtitle}</span>
            </dd>
            <dt>Progress</dt>
            <dd>{item.progress}</dd>
          </dl>
          {item.span === undefined ? null : <SpanValue span={item.span} help={spanHelp} />}
          {item.hash === undefined ? null : (
            <HashRow
              label={item.hash.label}
              under={item.hash.text}
              value={item.hash.value}
              name={`${item.hash.label}, ${item.hash.text}`}
              copy={copy}
            />
          )}
          {item.notes.map((note) => (
            <p key={note} class={styles["muted"]}>
              {note}
            </p>
          ))}
          <div class={styles["actions"]}>
            {item.actions.map((action) => {
              const { label, icon } = ACTIONS[action];
              return (
                <button
                  key={action}
                  type="button"
                  class={cx(styles["btn"], action === "remove" && styles["ghost"])}
                  onClick={() => {
                    onAction(action);
                  }}
                >
                  {icon === undefined ? null : <Icon name={icon} />}
                  {label}
                  <span class="visually-hidden"> {item.filename}</span>
                </button>
              );
            })}
          </div>
        </div>
      ) : null}
    </li>
  );
}

/**
 * The downloads tray: a panel at the right, newest download first, each a
 * card whose head opens its details. A download put above what the list
 * shows keeps the list in place and raises a pill pointing at it. Closed, it
 * is slid out and inert.
 */
export function DownloadTray({
  id,
  class: className,
  open,
  items,
  count,
  trayRef,
  onClose,
  onClear,
  onAction,
  copy,
  spanHelp,
}: {
  id: string;
  /** Where the panel sits in its parent. */
  class?: string | undefined;
  open: boolean;
  items: readonly TrayItem[];
  count: string;
  trayRef: Ref<HTMLElement>;
  onClose: () => void;
  onClear: () => void;
  onAction: (id: number, action: TrayAction) => void;
  copy: ((text: string) => Promise<void>) | undefined;
  spanHelp: { label: string; title: string; text: string };
}) {
  const list = useRef<HTMLOListElement>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(new Set());
  const [pill, setPill] = useState(false);
  // The first item and its offset at the last render, to keep the list in place when items go above it.
  const last = useRef<{ first: number | undefined; top: number }>({ first: undefined, top: 0 });

  useLayoutEffect(() => {
    const element = list.current;
    if (element === null) return;
    const { first, top } = last.current;
    if (first !== undefined && items[0]?.id !== first) {
      const anchor = element.querySelector<HTMLElement>(`[data-id="${first}"]`);
      if (anchor !== null) {
        const place = keepPlace(element.scrollTop, anchor.offsetTop - top, open);
        element.scrollTop = place.top;
        if (place.pill) setPill(true);
      }
    }
    const head = element.firstElementChild;
    last.current = { first: items[0]?.id, top: head instanceof HTMLElement ? head.offsetTop : 0 };
  });

  useEffect(() => {
    if (!open) setPill(false);
  }, [open]);

  return (
    <aside ref={trayRef} id={id} class={cx(styles["tray"], open && styles["shown"], className)} aria-label="Downloads" inert={!open}>
      <div class={styles["top"]}>
        <h2 class={styles["title"]}>Downloads</h2>
        <span class={styles["count"]}>{count}</span>
        <button type="button" class={cx(styles["btn"], styles["ghost"], styles["clear"])} onClick={onClear}>
          Clear finished
        </button>
        <button type="button" class={cx(styles["btn"], styles["ghost"])} aria-label="Close downloads" onClick={onClose}>
          <Icon name="close" />
        </button>
      </div>
      {pill ? (
        <button
          type="button"
          class={styles["pill"]}
          onClick={() => {
            if (list.current !== null) list.current.scrollTop = 0;
            setPill(false);
          }}
        >
          New download above
        </button>
      ) : null}
      <ol
        ref={list}
        class={styles["list"]}
        onScroll={(event) => {
          if (event.currentTarget.scrollTop <= TOP_SLACK) setPill(false);
        }}
      >
        {items.length === 0 ? (
          <li class={styles["empty"]}>No downloads. Downloads are started from a region's inspector.</li>
        ) : (
          items.map((item) => (
            <Item
              key={item.id}
              item={item}
              open={expanded.has(item.id)}
              onToggle={() => {
                setExpanded((current) => {
                  const next = new Set(current);
                  if (!next.delete(item.id)) next.add(item.id);
                  return next;
                });
              }}
              onAction={(action) => {
                onAction(item.id, action);
              }}
              copy={copy}
              spanHelp={spanHelp}
            />
          ))
        )}
      </ol>
    </aside>
  );
}
