import type { ComponentChildren, RefObject } from "preact";
import { Icon } from "./Icon.tsx";
import styles from "./splitpane.module.css";

/**
 * A list (master) beside its detail, each scrolling on its own. On a phone
 * one shows at a time: the detail slides over the list, under a bar with a
 * Back button, and the hidden one is inert.
 */
export function SplitPane({
  phone,
  shown,
  open,
  master,
  detail,
  back,
  detailRef,
  backRef,
}: {
  phone: boolean;
  /** Whether the detail is slid in, on a phone. */
  shown: boolean;
  /** Whether the detail is slid in or sliding in, on a phone; the list is hidden only once it is. */
  open: boolean;
  master: ComponentChildren;
  detail: ComponentChildren;
  back: { label: string; crumb: string; onBack: () => void };
  detailRef: RefObject<HTMLDivElement | null>;
  backRef: RefObject<HTMLButtonElement | null>;
}) {
  return (
    <div class={styles["panes"]}>
      <div class={styles["split"]} data-shown={shown ? "" : undefined}>
        <div class={styles["master"]} inert={phone && shown}>
          {master}
        </div>
        <div ref={detailRef} class={styles["detail"]} inert={phone && !open}>
          <div class={styles["bar"]}>
            <button ref={backRef} type="button" class={styles["back"]} onClick={back.onBack}>
              <Icon name="back" />
              {back.label}
            </button>
            <span class={styles["crumb"]}>{back.crumb}</span>
          </div>
          {detail}
        </div>
      </div>
    </div>
  );
}
