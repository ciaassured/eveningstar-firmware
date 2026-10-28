import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

type Theme = Record<string, string>;

function declarations(css: string): Theme {
  const tokens: Theme = {};
  for (const [, name, value] of css.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
    if (name !== undefined && value !== undefined) tokens[name] = value.trim();
  }
  return tokens;
}

const css = readFileSync(new URL("./tokens.css", import.meta.url), "utf8");
const dark = css.indexOf("@media (prefers-color-scheme: dark)");
assert.ok(dark > 0, "tokens.css has a dark theme");
const light = declarations(css.slice(0, dark));
const night = { ...light, ...declarations(css.slice(dark)) };
const themes: [string, Theme][] = [
  ["light", light],
  ["dark", night],
];

function channel(hex: string): number {
  const value = Number.parseInt(hex, 16) / 255;
  return value <= 0.039_28 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

function luminance(color: string): number {
  const match = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color);
  assert.ok(match, `${color} is a #rrggbb colour`);
  const [, r = "", g = "", b = ""] = match;
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return ((hi ?? 0) + 0.05) / ((lo ?? 0) + 0.05);
}

function token(theme: Theme, name: string): string {
  const value = theme[name];
  assert.ok(value, `${name} is defined`);
  return value;
}

/** Text: 4.5:1. */
const TEXT: [string, string][] = [
  ["--fg", "--bg"],
  ["--fg", "--surface"],
  ["--fg", "--surface-2"],
  ["--fg", "--nav-active"],
  ["--muted", "--bg"],
  ["--muted", "--surface"],
  ["--muted", "--surface-2"],
  ["--accent", "--bg"],
  ["--accent", "--surface"],
  ["--accent-fg", "--accent"],
  ["--sun", "--surface"],
  ["--sun", "--bg"],
  ["--moon", "--surface"],
  ["--moon", "--bg"],
  ["--header-fg", "--header-bg"],
  ["--header-muted", "--header-bg"],
  // The age of stale data turns this colour.
  ["--st-caution", "--surface"],
  ["--st-caution", "--bg"],
];

const STATUS = [
  "--st-off",
  "--st-standby",
  "--st-normal",
  "--st-caution",
  "--st-serious",
  "--st-critical",
];

/** Graphics and the edges of controls: 3:1. */
const GRAPHICS: [string, string][] = [
  ...STATUS.flatMap((status): [string, string][] => [
    [status, "--surface"],
    [status, "--bg"],
  ]),
  ["--sun-fill", "--surface"],
  ["--sun-fill", "--surface-2"],
  ["--moon", "--surface"],
  ["--accent", "--surface"],
  ["--accent", "--bg"],
  ["--st-normal", "--header-bg"],
  ["--st-caution", "--header-bg"],
  ["--st-critical", "--header-bg"],
  ["--moon", "--header-bg"],
  ["--sun-fill", "--header-bg"],
];

for (const [name, theme] of themes) {
  test(`${name} theme: text pairs meet 4.5:1`, () => {
    for (const [fg, bg] of TEXT) {
      const ratio = contrast(token(theme, fg), token(theme, bg));
      assert.ok(ratio >= 4.5, `${fg} on ${bg} is ${ratio.toFixed(2)}:1`);
    }
  });

  test(`${name} theme: graphics meet 3:1`, () => {
    for (const [fg, bg] of GRAPHICS) {
      const ratio = contrast(token(theme, fg), token(theme, bg));
      assert.ok(ratio >= 3, `${fg} on ${bg} is ${ratio.toFixed(2)}:1`);
    }
  });
}

test("both themes define the same tokens", () => {
  const colours = (theme: Theme) => Object.keys(theme).filter((key) => /^#/.test(theme[key] ?? "")).sort();
  assert.deepEqual(colours(declarations(css.slice(dark))), colours(declarations(css.slice(0, dark))));
});

test("the contrast of black on white is 21:1", () => {
  assert.equal(Math.round(contrast("#000000", "#ffffff")), 21);
});
