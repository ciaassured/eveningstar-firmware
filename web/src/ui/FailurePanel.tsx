import { Banner } from "./Banner.tsx";
import { Actions, Button } from "./Button.tsx";
import styles from "./failure.module.css";

export type FailureAction = "retry" | "reload" | "sign-in";

const LABELS: Record<FailureAction, string> = {
  retry: "Try again",
  reload: "Reload the page",
  "sign-in": "Sign in",
};

/**
 * Why something couldn't be shown or done, what probably caused it, and what
 * to do next. The details are for support and auditing.
 */
export function FailurePanel({
  title,
  explanation,
  actions,
  details,
  retryInS,
  onAction,
}: {
  title: string;
  explanation: string;
  actions: readonly FailureAction[];
  details: readonly string[];
  /** Seconds until the next automatic attempt, if there is one. */
  retryInS?: number | undefined;
  onAction: (action: FailureAction) => void;
}) {
  return (
    <Banner level="serious" title={title} urgent>
      <p>{explanation}</p>
      {retryInS === undefined ? null : <p class={styles["countdown"]}>Trying again in {retryInS} s.</p>}
      {actions.length > 0 ? (
        <Actions>
          {actions.map((action) => (
            <Button key={action} variant={action === actions[0] ? "primary" : "secondary"} onClick={() => onAction(action)}>
              {LABELS[action]}
            </Button>
          ))}
        </Actions>
      ) : null}
      <details class={styles["details"]}>
        <summary>Details</summary>
        <pre class={styles["pre"]}>{details.join("\n")}</pre>
      </details>
    </Banner>
  );
}
