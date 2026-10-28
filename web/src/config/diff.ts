import { isObject, jsonEqual } from "../lib/json.ts";
import type { JsonValue } from "../lib/json.ts";

/** Where in the configuration: the keys and array indexes down to a value. */
export type ConfigPath = readonly (string | number)[];

export type Change =
  | { kind: "added"; path: ConfigPath; after: JsonValue }
  | { kind: "removed"; path: ConfigPath; before: JsonValue }
  | { kind: "changed"; path: ConfigPath; before: JsonValue; after: JsonValue }
  /** A secret was set or replaced. Its value is never held in the change. */
  | { kind: "secret"; path: ConfigPath };

export interface DiffOptions {
  /** Whether the value at `path` is a write-only secret. */
  isSecret?: (path: ConfigPath) => boolean;
}

/** What differs between the running configuration and the candidate, in order of path. */
export function diffConfig(
  base: JsonValue | undefined,
  candidate: JsonValue | undefined,
  options: DiffOptions = {},
  path: ConfigPath = [],
): Change[] {
  if (jsonEqual(base, candidate)) return [];
  if (options.isSecret?.(path) === true) return [{ kind: "secret", path }];

  if (isObject(base) && isObject(candidate)) {
    const keys = [...new Set([...Object.keys(base), ...Object.keys(candidate)])].sort();
    return keys.flatMap((key) => diffConfig(base[key], candidate[key], options, [...path, key]));
  }
  if (Array.isArray(base) && Array.isArray(candidate)) {
    const length = Math.max(base.length, candidate.length);
    return Array.from({ length }, (_, index) =>
      diffConfig(base[index], candidate[index], options, [...path, index]),
    ).flat();
  }
  if (base === undefined && candidate !== undefined) return [{ kind: "added", path, after: candidate }];
  if (candidate === undefined && base !== undefined) return [{ kind: "removed", path, before: base }];
  if (base !== undefined && candidate !== undefined) {
    return [{ kind: "changed", path, before: base, after: candidate }];
  }
  return [];
}

function name(path: ConfigPath): string {
  return path.length === 0 ? "(configuration)" : path.map(String).join(".");
}

/** The changes as lines to read, in the way of `show | compare`: `-` for what goes, `+` for what comes. */
export function formatChanges(changes: readonly Change[]): string[] {
  return changes.flatMap((change): string[] => {
    const at = name(change.path);
    switch (change.kind) {
      case "added":
        return [`+ ${at}: ${JSON.stringify(change.after)}`];
      case "removed":
        return [`- ${at}: ${JSON.stringify(change.before)}`];
      case "changed":
        return [`- ${at}: ${JSON.stringify(change.before)}`, `+ ${at}: ${JSON.stringify(change.after)}`];
      case "secret":
        return [`~ ${at}: secret changed`];
    }
  });
}
