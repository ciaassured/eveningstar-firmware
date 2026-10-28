// Turns Vite's output into the single file the firmware serves: one
// index.html with exactly one inline module script and one inline style, plus
// the Content-Security-Policy that admits exactly those two by hash.
//
// usage: node inline.mjs <dist dir> <output dir>

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [distDir, outDir] = process.argv.slice(2);
if (!distDir || !outDir) {
  console.error("usage: inline.mjs <dist dir> <output dir>");
  process.exit(2);
}

function fail(message) {
  console.error(`inline: ${message}`);
  process.exit(1);
}

function read(path) {
  return readFileSync(join(distDir, path), "utf8");
}

const files = readdirSync(distDir, { recursive: true, withFileTypes: true })
  .filter((entry) => entry.isFile())
  .map((entry) => join(entry.parentPath, entry.name).slice(distDir.length + 1));
const referenced = new Set(["index.html"]);

let html = read("index.html");

function takeOne(pattern, what) {
  const matches = [...html.matchAll(pattern)];
  if (matches.length !== 1) {
    fail(`expected exactly one ${what}, found ${matches.length}`);
  }
  const [match] = matches;
  if (!match || match.index === undefined) fail(`no ${what}`);
  return match;
}

const scriptTag = takeOne(
  /<script\b[^>]*\btype="module"[^>]*\bsrc="([^"]+)"[^>]*><\/script>/g,
  "module script",
);
const styleTag = takeOne(
  /<link\b[^>]*\brel="stylesheet"[^>]*\bhref="([^"]+)"[^>]*>/g,
  "stylesheet link",
);

function bodyOf(url, what) {
  if (!url.startsWith("/") || url.startsWith("//")) {
    fail(`${what} URL ${url} is not a path on this origin`);
  }
  const path = url.slice(1);
  referenced.add(path);
  return read(path);
}

const script = bodyOf(scriptTag[1] ?? "", "script");
const style = bodyOf(styleTag[1] ?? "", "stylesheet");

if (/<\/script/i.test(script) || /<!--/.test(script)) {
  fail("the script contains a sequence that could end its element early");
}
if (/<\/style/i.test(style)) {
  fail("the stylesheet contains a sequence that could end its element early");
}

// Replace with functions so `$` in the code is not read as a pattern.
html = html.replace(scriptTag[0], () => "");
html = html.replace(styleTag[0], () => `<style>${style}</style>`);
html = html.replace("</body>", () => `<script type="module">${script}</script>\n  </body>`);

const unreferenced = files.filter((file) => !referenced.has(file));
if (unreferenced.length > 0) {
  fail(`build output not inlined: ${unreferenced.join(", ")}`);
}

function count(pattern) {
  return [...html.matchAll(pattern)].length;
}
if (count(/<script\b/gi) !== 1 || count(/<script type="module">/g) !== 1) {
  fail("the page must hold exactly one inline module script");
}
if (count(/<style\b/gi) !== 1) fail("the page must hold exactly one inline style");
if (count(/<link\b[^>]*\brel="(?:stylesheet|modulepreload)"/gi) !== 0) {
  fail("the page must not link stylesheets or preload modules");
}

// Outside the two inline bodies, only data: URLs may be referenced.
const markup = html.replace(/<script[\s\S]*?<\/script>/g, "").replace(/<style[\s\S]*?<\/style>/g, "");
for (const [, , url] of markup.matchAll(/\b(src|href|action|poster|srcset)="([^"]*)"/g)) {
  if (!url?.startsWith("data:")) fail(`the markup references ${url}`);
}

function hash(text) {
  return `'sha256-${createHash("sha256").update(text, "utf8").digest("base64")}'`;
}

const policy = [
  "default-src 'none'",
  `script-src ${hash(script)}`,
  `style-src ${hash(style)}`,
  "connect-src 'self'",
  "img-src 'self' data:",
  "base-uri 'none'",
  "form-action 'self'",
  "frame-ancestors 'none'",
  "require-trusted-types-for 'script'",
  "trusted-types 'none'",
].join("; ");

mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, "index.html"), html);
writeFileSync(join(outDir, "content-security-policy"), `${policy}\n`);
