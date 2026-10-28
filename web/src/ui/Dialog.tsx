import type { ComponentChildren } from "preact";
import { useEffect, useId, useRef } from "preact/hooks";
import styles from "./dialog.module.css";

/**
 * A modal dialog on the browser's own `<dialog>`, so focus is trapped, Escape
 * closes it and the page behind is inert.
 */
export function Dialog({
  open,
  title,
  onClose,
  children,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ComponentChildren;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();

  useEffect(() => {
    const dialog = ref.current;
    if (dialog === null) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      class={styles["dialog"]}
      aria-labelledby={titleId}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      {open ? (
        <div class={styles["body"]}>
          <h2 class={styles["title"]} id={titleId}>
            {title}
          </h2>
          {children}
        </div>
      ) : null}
    </dialog>
  );
}
