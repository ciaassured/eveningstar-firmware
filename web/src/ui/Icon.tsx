import { cx } from "../lib/cx.ts";
import styles from "./icon.module.css";

/** The stroked icons of the storage page and the downloads, on a 16 × 16 view box. */
export type IconName =
  | "ok"
  | "warn"
  | "bad"
  | "lock"
  | "next"
  | "rollback"
  | "back"
  | "copy"
  | "close"
  | "download"
  | "pending"
  | "failed"
  | "cancelled";

function Paths({ name }: { name: IconName }) {
  switch (name) {
    case "ok":
      return (
        <>
          <circle cx="8" cy="8" r="6.2" />
          <path d="m5.2 8.2 1.9 1.9 3.8-4" />
        </>
      );
    case "warn":
      return (
        <>
          <path d="M8 2.2 14.3 13H1.7z" />
          <path d="M8 6.4v3M8 11.1v.1" />
        </>
      );
    case "bad":
      return (
        <>
          <path d="M5.4 1.8h5.2l3.6 3.6v5.2l-3.6 3.6H5.4l-3.6-3.6V5.4z" />
          <path d="M8 5v3.6M8 10.8v.1" />
        </>
      );
    case "lock":
      return (
        <>
          <rect x="3.2" y="7" width="9.6" height="6.8" rx="1.2" />
          <path d="M5.4 7V5.2a2.6 2.6 0 0 1 5.2 0V7" />
        </>
      );
    case "next":
      return <path d="M2.5 8h9M8.5 4.5 12 8l-3.5 3.5M13.8 3.5v9" />;
    case "rollback":
      return (
        <>
          <path d="M3.2 7.5A5 5 0 1 1 4.8 11.8" />
          <path d="M2.8 3.8v3.9h3.9" />
        </>
      );
    case "back":
      return <path d="M10 3 5 8l5 5" />;
    case "copy":
      return (
        <>
          <rect x="5.5" y="5.5" width="8" height="8" rx="1.3" />
          <path d="M10.5 3.2V3a1 1 0 0 0-1-1H3.4a1 1 0 0 0-1 1v6.1a1 1 0 0 0 1 1h.4" />
        </>
      );
    case "close":
      return <path d="M4 4l8 8M12 4l-8 8" />;
    case "download":
      return <path d="M8 2v8.2M4.6 7 8 10.4 11.4 7M3 13.6h10" />;
    case "pending":
      return (
        <>
          <circle cx="8" cy="8" r="6.2" />
          <path d="M8 4.6V8l2.4 1.6" />
        </>
      );
    case "failed":
      return (
        <>
          <circle cx="8" cy="8" r="6.2" />
          <path d="m5.8 5.8 4.4 4.4M10.2 5.8l-4.4 4.4" />
        </>
      );
    case "cancelled":
      return (
        <>
          <circle cx="8" cy="8" r="6.2" />
          <path d="m3.7 12.3 8.6-8.6" />
        </>
      );
  }
}

/**
 * An icon beside text, hidden from assistive technology: the text beside it
 * carries its meaning. It is 16 px unless `size` says otherwise, and takes the
 * text's colour.
 */
export function Icon({ name, size = 16, class: className }: { name: IconName; size?: 12 | 14 | 16 | 18 | 20; class?: string | undefined }) {
  return (
    <svg class={cx(styles["icon"], styles[`s${size}`], className)} viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <Paths name={name} />
    </svg>
  );
}

/** The booted dot: 8 px, in the booted colour. */
export function Dot() {
  return <span class={styles["dot"]} aria-hidden="true" />;
}
