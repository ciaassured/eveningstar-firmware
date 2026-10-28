import type { ApiFailure, FailureContext } from "../api/failure.ts";
import { describeFailure, failureDetails } from "../api/failure.ts";
import { FailurePanel } from "../ui/FailurePanel.tsx";
import type { FailureAction } from "../ui/FailurePanel.tsx";
import { useServices } from "./context.ts";

/** Why a request failed, in words, with what can be done about it. */
export function FailureView({
  failure,
  context,
  retryInS,
  onRetry,
}: {
  failure: ApiFailure;
  context: FailureContext;
  retryInS?: number | undefined;
  onRetry?: (() => void) | undefined;
}) {
  const { router } = useServices();
  const message = describeFailure(failure);

  function act(action: FailureAction): void {
    switch (action) {
      case "retry":
        onRetry?.();
        return;
      case "reload":
        globalThis.location.reload();
        return;
      case "sign-in":
        router.requireLogin();
        return;
    }
  }

  return (
    <FailurePanel
      title={message.title}
      explanation={message.explanation}
      actions={message.actions.filter((action) => action !== "retry" || onRetry !== undefined)}
      details={failureDetails(failure, context)}
      retryInS={retryInS}
      onAction={act}
    />
  );
}
