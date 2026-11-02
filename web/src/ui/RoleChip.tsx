import { cx } from "../lib/cx.ts";
import { Dot, Icon } from "./Icon.tsx";
import styles from "./rolechip.module.css";

/** A boot role of an app slot, as a chip. */
export interface RoleChipProps {
  role: "running" | "selected" | "rollback_target" | "other";
  label: string;
  /** For `selected`: the solid pill of an activated update, or the plain chip with a warning for a fallback. */
  variant?: "plain" | "update" | "fallback" | undefined;
  /** The warning's tooltip, for a fallback. */
  tooltip?: string | undefined;
}

/** A role chip: the booted dot, the next-boot icon (a solid pill for an update) or the rollback icon, then the word. */
export function RoleChip({ role, label, variant, tooltip }: RoleChipProps) {
  const icon =
    role === "running" ? <Dot /> : role === "selected" ? <Icon name="next" size={14} /> : role === "rollback_target" ? <Icon name="rollback" size={14} /> : null;
  return (
    <span class={cx(styles["chip"], variant === "update" && styles["update"])}>
      {icon}
      {label}
      {variant === "fallback" ? (
        <span class={styles["fallback"]} title={tooltip}>
          <Icon name="warn" size={14} />
          <span class="visually-hidden">{tooltip}</span>
        </span>
      ) : null}
    </span>
  );
}
