import type { ComponentChildren } from "preact";
import { useEffect, useMemo } from "preact/hooks";
import { FailureView } from "./FailureView.tsx";
import { useServices } from "./context.ts";
import { createLoadController } from "./load-controller.ts";
import type { Loader } from "./load-controller.ts";

/**
 * Shows its children once every loader has succeeded. Until then it shows
 * that it is loading, or why it couldn't. A view never shows with part of its
 * data missing.
 */
export function LoadGate({
  group,
  loaders,
  children,
}: {
  /** The page; leaving it aborts the loads. */
  group: string;
  loaders: readonly Loader[];
  children: ComponentChildren;
}) {
  const { transport, timers } = useServices();
  const controller = useMemo(
    () =>
      createLoadController({
        loaders,
        timers,
        abort: () => {
          transport.abortGroup(group);
        },
      }),
    // The loaders of a page don't change while it is shown.
    [group],
  );

  useEffect(() => {
    void controller.load();
    return () => {
      controller.dispose();
    };
  }, [controller]);

  const state = controller.state.value;
  switch (state.kind) {
    case "ready":
      return <>{children}</>;
    case "failed":
      return (
        <FailureView
          failure={state.failure}
          context={state.context}
          retryInS={state.retryInS}
          onRetry={() => {
            controller.retry();
          }}
        />
      );
    case "loading":
      return <p role="status">Loading…</p>;
  }
}
