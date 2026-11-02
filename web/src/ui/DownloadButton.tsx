import type { ComponentChildren } from "preact";
import { cx } from "../lib/cx.ts";
import styles from "./downloadbutton.module.css";
import { Icon } from "./Icon.tsx";
import type { IconName } from "./Icon.tsx";

/**
 * A download button: the download icon, then a title over a sublabel. Just
 * after a click it reads `addedText` with a border in the completed colour.
 */
export function DownloadButton({
  title,
  sublabel,
  primary,
  disabled,
  added,
  addedText,
  onClick,
}: {
  title: string;
  sublabel: string;
  primary: boolean;
  disabled: boolean;
  added: boolean;
  addedText: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      class={cx(styles["button"], primary && styles["primary"], added && styles["added"])}
      disabled={disabled}
      onClick={onClick}
    >
      <Icon name="download" />
      <span>
        <b class={styles["title"]} aria-live="polite">
          {added ? addedText : title}
        </b>
        <small class={styles["sublabel"]}>{sublabel}</small>
      </span>
    </button>
  );
}

/** Download buttons in a wrapping row, all the same height. */
export function DownloadActions({ label, children }: { label: string; children: ComponentChildren }) {
  return (
    <div class={styles["actions"]} role="group" aria-label={label}>
      {children}
    </div>
  );
}

/** A note in place of the downloads: muted text with an optional icon, in a dashed box. */
export function NoteBox({ icon, children }: { icon?: IconName | null | undefined; children: ComponentChildren }) {
  return (
    <div class={styles["note"]}>
      {icon === null || icon === undefined ? null : <Icon name={icon} />}
      <span>{children}</span>
    </div>
  );
}
