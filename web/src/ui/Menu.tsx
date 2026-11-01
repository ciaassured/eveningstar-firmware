import { useRef } from "preact/hooks";
import styles from "./menu.module.css";

export interface MenuItem {
  key: string;
  label: string;
  /** A line under the label, such as a size. */
  hint?: string | undefined;
}

/**
 * A button that opens a short list of choices, on the browser's own
 * disclosure element, so it opens with Enter or Space and needs no script to
 * stay accessible. Choosing closes it.
 */
export function MenuButton({
  label,
  accessibleLabel,
  items,
  onSelect,
}: {
  label: string;
  /** The button's name for assistive technology, when `label` alone is too short. */
  accessibleLabel?: string | undefined;
  items: readonly MenuItem[];
  onSelect: (key: string) => void;
}) {
  const ref = useRef<HTMLDetailsElement>(null);
  return (
    <details
      ref={ref}
      class={styles["menu"]}
      onKeyDown={(event) => {
        if (event.key === "Escape" && ref.current !== null) {
          ref.current.open = false;
          ref.current.querySelector("summary")?.focus();
        }
      }}
    >
      <summary class={styles["summary"]} aria-label={accessibleLabel}>
        {label}
        <svg class={styles["caret"]} viewBox="0 0 10 10" aria-hidden="true" focusable="false">
          <path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" stroke-width="1.5" />
        </svg>
      </summary>
      <ul class={styles["list"]}>
        {items.map((item) => (
          <li key={item.key}>
            <button
              type="button"
              class={styles["item"]}
              onClick={() => {
                if (ref.current !== null) ref.current.open = false;
                onSelect(item.key);
              }}
            >
              <span>{item.label}</span>
              {item.hint === undefined ? null : <span class={styles["hint"]}>{item.hint}</span>}
            </button>
          </li>
        ))}
      </ul>
    </details>
  );
}
