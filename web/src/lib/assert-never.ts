/** Marks a point the compiler proves unreachable, such as the end of an exhaustive `switch`. */
export function assertNever(value: never): never {
  throw new Error(`unreachable: ${String(value)}`);
}
