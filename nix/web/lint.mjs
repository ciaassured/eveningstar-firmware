// Checks the web UI's supply chain and source against the rules in
// specs/web-ui.md, using only Node, so linting adds no packages.
//
// usage: node lint.mjs <repository root>

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

const root = process.argv[2];
if (!root) {
  console.error("usage: lint.mjs <repository root>");
  process.exit(2);
}

const problems = [];
function problem(where, message) {
  problems.push(`${where}: ${message}`);
}

const trees = ["web", "tools/openapi-typescript"];
const registry = "https://registry.npmjs.org/";
// Optional platform packages that run an install script on their own platform.
const installScriptAllowlist = new Set(["node_modules/fsevents"]);

const npmrc = [
  "ignore-scripts=true",
  "save-exact=true",
  "allow-git=none",
  "min-release-age=7",
  "engine-strict=true",
  "audit=true",
  "fund=false",
];

const lifecycle = new Set([
  "preinstall",
  "install",
  "postinstall",
  "prepublish",
  "preprepare",
  "prepare",
  "postprepare",
  "prepack",
  "postpack",
]);

function lintNpmrc(tree) {
  const where = `${tree}/.npmrc`;
  const lines = readFileSync(join(root, where), "utf8")
    .split("\n")
    .filter((line) => line.trim() !== "" && !line.startsWith("#"));
  for (const wanted of npmrc) {
    if (!lines.includes(wanted)) problem(where, `missing ${wanted}`);
  }
  for (const line of lines) {
    if (!npmrc.includes(line)) problem(where, `unexpected setting ${line}`);
  }
}

function lintPackageJson(tree) {
  const where = `${tree}/package.json`;
  const pkg = JSON.parse(readFileSync(join(root, where), "utf8"));
  if (pkg.private !== true) problem(where, "must be private");
  for (const name of Object.keys(pkg.scripts ?? {})) {
    if (lifecycle.has(name)) problem(where, `has the lifecycle script ${name}`);
  }
  for (const field of ["engines"]) {
    for (const key of ["node", "npm"]) {
      if (!/^\d+\.\d+\.\d+$/.test(pkg[field]?.[key] ?? "")) {
        problem(where, `${field}.${key} must be an exact version`);
      }
    }
  }
  for (const field of ["dependencies", "devDependencies", "optionalDependencies"]) {
    for (const [name, version] of Object.entries(pkg[field] ?? {})) {
      if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
        problem(where, `${field}.${name} is ${version}, not an exact version`);
      }
    }
  }
  for (const field of ["peerDependencies", "bundledDependencies", "overrides"]) {
    if (pkg[field] !== undefined) problem(where, `${field} is not allowed`);
  }
}

function lintLockfile(tree) {
  const where = `${tree}/package-lock.json`;
  const lock = JSON.parse(readFileSync(join(root, where), "utf8"));
  if (lock.lockfileVersion !== 3) problem(where, "lockfileVersion must be 3");
  for (const [path, entry] of Object.entries(lock.packages ?? {})) {
    if (path === "") continue;
    const name = `${where} ${path}`;
    if (entry.link) problem(name, "is a link");
    const resolved = entry.resolved ?? "";
    if (!resolved.startsWith(registry) || !resolved.endsWith(".tgz")) {
      problem(name, `resolves to ${resolved || "nothing"}, not a registry tarball`);
    }
    if (!/^sha512-/.test(entry.integrity ?? "")) problem(name, "has no sha512 integrity");
    if (entry.hasInstallScript && !installScriptAllowlist.has(path)) {
      problem(name, "has an install script");
    }
  }
}

function sourceFiles(dir) {
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && /\.(?:[cm]?[jt]sx?|html|css)$/.test(entry.name))
    .map((entry) => join(entry.parentPath, entry.name))
    .filter((file) => !relative(dir, file).startsWith(join("api", "generated") + "/"));
}

const banned = [
  [/\binnerHTML\b/, "innerHTML"],
  [/\bouterHTML\b/, "outerHTML"],
  [/\binsertAdjacentHTML\b/, "insertAdjacentHTML"],
  [/\bdocument\.write\b/, "document.write"],
  [/\bdangerouslySetInnerHTML\b/, "dangerouslySetInnerHTML"],
  [/\beval\s*\(/, "eval("],
  [/\bnew\s+Function\b/, "new Function"],
  [/\bset(?:Timeout|Interval)\s*\(\s*["'`]/, "a string argument to setTimeout or setInterval"],
  [/javascript:/, "javascript:"],
  [/:\s*any\b/, ": any"],
  [/\bas\s+any\b/, "as any"],
  [/@ts-ignore/, "@ts-ignore"],
];

const src = join(root, "web", "src");
for (const file of sourceFiles(src)) {
  const rel = relative(root, file);
  const text = readFileSync(file, "utf8");
  for (const [pattern, name] of banned) {
    if (pattern.test(text)) problem(rel, `uses ${name}`);
  }
  if (/\b(?:localStorage|sessionStorage)\b/.test(text) && rel !== join("web", "src", "lib", "storage.ts")) {
    problem(rel, "uses web storage outside src/lib/storage.ts");
  }
  if (relative(src, file).startsWith("ui/")) {
    for (const [, specifier] of text.matchAll(/\b(?:from|import)\s*\(?\s*["'](\.[^"']*)["']/g)) {
      const target = relative(src, resolve(dirname(file), specifier ?? ""));
      if (!target.startsWith("ui/") && !target.startsWith("lib/")) {
        problem(rel, `imports ${specifier}, but ui/ may only import lib/ and ui/`);
      }
    }
  }
}

for (const tree of trees) {
  lintNpmrc(tree);
  lintPackageJson(tree);
  lintLockfile(tree);
}

if (problems.length > 0) {
  console.error(problems.join("\n"));
  console.error(`\n${problems.length} problem(s)`);
  process.exit(1);
}
console.log("web lint: ok");
