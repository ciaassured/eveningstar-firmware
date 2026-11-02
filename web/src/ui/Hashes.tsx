import type { ComponentChildren } from "preact";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { copyOrSelect } from "../lib/clipboard.ts";
import type { CopyOutcome } from "../lib/clipboard.ts";
import styles from "./hashes.module.css";
import { Icon } from "./Icon.tsx";
import { HelpButton } from "./Popover.tsx";

/**
 * The hashes of a region under a line: the title, the note and the rows. With
 * `lead`, the note follows the title on its line, above the rows; otherwise it
 * follows the rows.
 */
export function Hashes({ title, note, lead, children }: { title: string; note: string; lead: boolean; children?: ComponentChildren }) {
  return (
    <div class={styles["section"]}>
      {lead ? (
        <div>
          <h4 class={styles["title"]}>{title}</h4>
          <span class={styles["note"]}>{note}</span>
        </div>
      ) : (
        <h4 class={styles["title"]}>{title}</h4>
      )}
      {children === undefined ? null : <div class={styles["rows"]}>{children}</div>}
      {lead ? null : <p class={styles["note"]}>{note}</p>}
    </div>
  );
}

const SAID: Record<CopyOutcome, { word: string; ms: number }> = {
  copied: { word: "Copied", ms: 1500 },
  selected: { word: "Selected", ms: 2500 },
};

function selectAll(field: HTMLTextAreaElement): void {
  field.select();
  field.setSelectionRange(0, field.value.length);
}

/**
 * A hash: its label with the text under it, the value in a read-only field
 * that a click selects whole, and a Copy button. The button writes the value
 * to the clipboard, or, where the page has none (`copy` undefined) or it
 * refuses, focuses and selects the field for the person to copy.
 */
export function HashRow({
  label,
  under,
  value,
  name,
  help,
  copy,
}: {
  label: string;
  under: string;
  value: string;
  /** The field's accessible name. */
  name: string;
  help?: { label: string; title: string; text: string } | undefined;
  copy: ((text: string) => Promise<void>) | undefined;
}) {
  const field = useRef<HTMLTextAreaElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const [said, setSaid] = useState<CopyOutcome | undefined>(undefined);

  useEffect(() => () => {
    clearTimeout(timer.current);
  }, []);

  // The field grows to its content; where `field-sizing` isn't supported, by its scroll height.
  useLayoutEffect(() => {
    const element = field.current;
    if (element === null || CSS.supports("field-sizing", "content")) return;
    let width = -1;
    const fit = (): void => {
      if (element.clientWidth === width) return;
      width = element.clientWidth;
      element.style.setProperty("height", "auto");
      element.style.setProperty("height", `${element.scrollHeight + 2}px`);
    };
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, [value]);

  const onCopy = (): void => {
    void copyOrSelect(value, copy, () => {
      const element = field.current;
      if (element === null) return;
      element.focus();
      selectAll(element);
    }).then((outcome) => {
      clearTimeout(timer.current);
      setSaid(outcome);
      timer.current = setTimeout(() => {
        setSaid(undefined);
      }, SAID[outcome].ms);
    });
  };

  return (
    <div class={styles["row"]}>
      <div class={styles["key"]}>
        <span class={styles["label"]}>
          <b>{label}</b>
          {help === undefined ? null : <HelpButton {...help} />}
        </span>
        <span class={styles["under"]}>{under}</span>
      </div>
      <textarea
        ref={field}
        class={styles["field"]}
        readOnly
        rows={1}
        spellcheck={false}
        aria-label={name}
        value={value}
        onClick={(event) => {
          selectAll(event.currentTarget);
        }}
      />
      <button type="button" class={styles["copy"]} onClick={onCopy}>
        <Icon name="copy" />
        <span aria-live="polite">{said === undefined ? "Copy" : SAID[said].word}</span>
        <span class="visually-hidden"> {label}</span>
      </button>
    </div>
  );
}
