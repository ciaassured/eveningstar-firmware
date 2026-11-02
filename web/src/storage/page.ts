import type { ReadonlySignal } from "@preact/signals";
import { batch, signal } from "@preact/signals";
import type { Timers } from "../api/transport.ts";
import { keepSelection } from "./items.ts";
import type { StorageModel } from "./items.ts";

/** How long a download button reads "Added to downloads" after a click. */
export const ADDED_MS = 1_600;

/**
 * Where the inspector is on a phone, where it slides over the list:
 * - `closed`: off-screen, the list shown;
 * - `opening`: drawn off-screen with its new content, to slide in on the next frames;
 * - `open`: slid in over the list.
 * On a desktop both are shown, and a selection opens it, so that a narrower window shows the inspector.
 */
export type Slide = "closed" | "opening" | "open";

export interface StoragePageDeps {
  timers: Timers;
  /** Calls `callback` before the next frame is drawn. */
  frame: (callback: () => void) => void;
}

/** What the storage page shows apart from the device's data: the selection, the phone's slide and the downloads just added. */
export interface StoragePage {
  /** The key of the selected item. */
  readonly selected: ReadonlySignal<string | undefined>;
  readonly slide: ReadonlySignal<Slide>;
  /** The key of the download button clicked last, while it reads "Added to downloads". */
  readonly added: ReadonlySignal<string | undefined>;
  /** Takes in a new model: the selection is kept by its key, or falls back to the running app's region. */
  sync(model: StorageModel): void;
  /**
   * Selects the item. On a phone with the inspector closed, the inspector is
   * drawn closed with the new item first and opens two frames later, so it
   * slides in.
   */
  select(id: string, phone: boolean): void;
  /** Slides the inspector out, on a phone. */
  back(): void;
  /** Notes a click on a download button, which reads "Added to downloads" for a while. */
  markAdded(key: string): void;
  dispose(): void;
}

export function createStoragePage(deps: StoragePageDeps): StoragePage {
  const selected = signal<string | undefined>(undefined);
  const slide = signal<Slide>("closed");
  const added = signal<string | undefined>(undefined);
  let addedTimer: unknown;
  let disposed = false;

  return {
    selected,
    slide,
    added,
    sync(model) {
      selected.value = keepSelection(model, selected.peek());
    },
    select(id, phone) {
      batch(() => {
        selected.value = id;
        if (!phone) slide.value = "open";
        else if (slide.peek() === "closed") slide.value = "opening";
      });
      if (phone && slide.peek() === "opening") {
        deps.frame(() => {
          deps.frame(() => {
            if (!disposed && slide.peek() === "opening") slide.value = "open";
          });
        });
      }
    },
    back() {
      slide.value = "closed";
    },
    markAdded(key) {
      deps.timers.clearTimeout(addedTimer);
      added.value = key;
      addedTimer = deps.timers.setTimeout(() => {
        added.value = undefined;
      }, ADDED_MS);
    },
    dispose() {
      disposed = true;
      deps.timers.clearTimeout(addedTimer);
    },
  };
}
