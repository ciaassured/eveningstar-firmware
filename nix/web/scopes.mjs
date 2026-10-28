// Writes the scope each operation of the API needs, as the OpenAPI document
// names it in the operation's `security` requirement, as a TypeScript module.
// The document is the one place the scopes are declared.
//
// usage: node scopes.mjs <openapi document> <output file>

import { readFileSync, writeFileSync } from "node:fs";

const [documentPath, outFile] = process.argv.slice(2);
if (!documentPath || !outFile) {
  console.error("usage: scopes.mjs <openapi document> <output file>");
  process.exit(2);
}

function fail(message) {
  console.error(`scopes: ${message}`);
  process.exit(1);
}

const document = JSON.parse(readFileSync(documentPath, "utf8"));
const scheme = "bearerAuth";
if (document.components?.securitySchemes?.[scheme] === undefined) {
  fail(`the document has no ${scheme} security scheme`);
}
const known = new Set((document.components?.schemas?.Scope?.oneOf ?? []).map((variant) => variant.const));
if (known.size === 0) fail("the document has no Scope schema");

const methods = ["get", "put", "post", "delete", "patch", "options", "head", "trace"];
const needed = {};
for (const [path, item] of Object.entries(document.paths ?? {})) {
  for (const method of methods) {
    const operation = item[method];
    if (operation === undefined) continue;
    const id = operation.operationId;
    if (typeof id !== "string") fail(`${method} ${path} has no operationId`);

    // One requirement of the bearer scheme, with one scope the document defines.
    const requirements = operation.security;
    const scopes = Array.isArray(requirements) && requirements.length === 1 ? requirements[0][scheme] : undefined;
    if (!Array.isArray(scopes) || scopes.length !== 1 || Object.keys(requirements[0]).length !== 1) {
      fail(`${id} must require exactly one scope of ${scheme}, and has ${JSON.stringify(requirements)}`);
    }
    if (!known.has(scopes[0])) fail(`${id} requires the scope ${JSON.stringify(scopes[0])}, which the document doesn't define`);
    needed[id] = scopes[0];
  }
}

const entries = Object.entries(needed)
  .sort(([a], [b]) => a.localeCompare(b))
  .map(([id, scope]) => `  ${id}: ${JSON.stringify(scope)},\n`)
  .join("");

writeFileSync(
  outFile,
  `// Written by nix/web/scopes.mjs from the OpenAPI document.
import type { components, operations } from "./v1.d.ts";

/** The scope a token needs to call each operation. */
export const OPERATION_SCOPE = {
${entries}} as const satisfies Record<keyof operations, components["schemas"]["Scope"]>;
`,
);
