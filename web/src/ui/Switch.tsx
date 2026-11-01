import styles from "./switch.module.css";

/** Two or more views of the same thing, one chosen at a time; the chosen one carries the selection bar. */
export function Switch<K extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: readonly { key: K; label: string }[];
  value: K;
  onChange: (key: K) => void;
}) {
  return (
    <div class={styles["switch"]} role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.key}
          type="button"
          class={styles["option"]}
          aria-pressed={option.key === value}
          onClick={() => {
            onChange(option.key);
          }}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
