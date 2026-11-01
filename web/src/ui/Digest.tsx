import { useEffect, useRef, useState } from "preact/hooks";
import { shortDigest } from "../lib/format.ts";
import styles from "./digest.module.css";

/**
 * A digest, shortened, with a button that copies it whole. The clipboard
 * exists only in a secure context; without one (`copy` undefined) the button
 * shows the digest whole and selects it, for the person to copy.
 */
export function CopyDigest({
  value,
  label,
  copy,
}: {
  value: string;
  /** What the digest is, for the button's accessible name, such as `ELF SHA-256`. */
  label: string;
  copy: ((text: string) => Promise<void>) | undefined;
}) {
  const [shown, setShown] = useState<"short" | "whole">("short");
  const [message, setMessage] = useState("");
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    const element = ref.current;
    if (shown !== "whole" || copy !== undefined || element === null) return;
    globalThis.getSelection()?.selectAllChildren(element);
  }, [shown, copy]);

  const act = (): void => {
    if (copy === undefined) {
      setShown("whole");
      setMessage("Selected; press Ctrl+C or ⌘C to copy");
      const element = ref.current;
      if (element !== null) globalThis.getSelection()?.selectAllChildren(element);
      return;
    }
    copy(value).then(
      () => {
        setMessage("Copied");
      },
      () => {
        setShown("whole");
        setMessage("Couldn't copy; select it to copy");
      },
    );
  };

  return (
    <span class={styles["digest"]}>
      <code ref={ref} class={shown === "whole" ? styles["whole"] : styles["short"]} title={value}>
        {shown === "whole" ? value : shortDigest(value)}
      </code>
      <button type="button" class={styles["button"]} onClick={act} aria-label={`${copy === undefined ? "Select" : "Copy"} ${label}`}>
        {copy === undefined ? "Select" : "Copy"}
      </button>
      <span class={styles["message"]} role="status">
        {message}
      </span>
    </span>
  );
}
