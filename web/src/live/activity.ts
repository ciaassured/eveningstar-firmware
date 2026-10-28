import type { ReadonlySignal } from "@preact/signals";
import { computed, signal } from "@preact/signals";
import type { Timers } from "../api/transport.ts";

/** After this long without input, polling pauses until the person resumes it. */
export const IDLE_AFTER_MS = 30 * 60_000;

/** What the activity tracker reads of the browser. */
export interface ActivityEnvironment {
  visible(): boolean;
  /** Calls `listener` when the page is shown or hidden. Returns a function that stops the calls. */
  onVisibilityChange(listener: () => void): () => void;
  /** Calls `listener` on input in the page. Returns a function that stops the calls. */
  onInput(listener: () => void): () => void;
  timers: Timers;
}

export interface Activity {
  readonly visible: ReadonlySignal<boolean>;
  readonly idle: ReadonlySignal<boolean>;
  /** Whether the page is shown and in use: whether live data should be fetched. */
  readonly active: ReadonlySignal<boolean>;
  /** Records input. Once the page is idle, only {@link Activity.resume} wakes it. */
  touch(): void;
  /** Ends the idle pause. */
  resume(): void;
  dispose(): void;
}

export function createActivity(environment: ActivityEnvironment): Activity {
  const { timers } = environment;
  const visible = signal(environment.visible());
  const idle = signal(false);
  let timer: unknown;

  const arm = (): void => {
    timers.clearTimeout(timer);
    timer = timers.setTimeout(() => {
      idle.value = true;
    }, IDLE_AFTER_MS);
  };
  arm();

  const stopVisibility = environment.onVisibilityChange(() => {
    visible.value = environment.visible();
  });
  const touch = (): void => {
    if (!idle.peek()) arm();
  };
  const stopInput = environment.onInput(touch);

  return {
    visible,
    idle,
    active: computed(() => visible.value && !idle.value),
    touch,
    resume() {
      idle.value = false;
      arm();
    },
    dispose() {
      timers.clearTimeout(timer);
      stopVisibility();
      stopInput();
    },
  };
}
