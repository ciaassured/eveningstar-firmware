// Writes the capability each operation of the API needs, as the OpenAPI
// document names it in `x-eveningstar-capability`, as a TypeScript module.
// The document is the one place the capabilities are declared.
//
// usage: node capabilities.mjs <openapi document> <output file>

import { readFileSync, writeFileSync } from "node:fs";

const [documentPath, outFile] = process.argv.slice(2);
if (!documentPath || !outFile) {
  console.error("usage: capabilities.mjs <openapi document> <output file>");
  process.exit(2);
}

const document = JSON.parse(readFileSync(documentPath, "utf8"));
const known = new Set(
  (document.components?.schemas?.Capability?.oneOf ?? []).map((variant) => variant.const),
);
if (known.size === 0) {
  console.error("capabilities: the document has no Capability schema");
  process.exit(1);
}

const methods = ["get", "put", "post", "delete", "patch", "options", "head", "trace"];
const needed = {};
for (const [path, item] of Object.entries(document.paths ?? {})) {
  for (const method of methods) {
    const operation = item[method];
    if (operation === undefined) continue;
    const id = operation.operationId;
    const capability = operation["x-eveningstar-capability"];
    if (typeof id !== "string") {
      console.error(`capabilities: ${method} ${path} has no operationId`);
      process.exit(1);
    }
    if (!known.has(capability)) {
      console.error(`capabilities: ${id} names the capability ${JSON.stringify(capability)}, which the document doesn't define`);
      process.exit(1);
    }
    needed[id] = capability;
  }
}

const entries = Object.entries(needed)
  .sort(([a], [b]) => a.localeCompare(b))
  .map(([id, capability]) => `  ${id}: ${JSON.stringify(capability)},\n`)
  .join("");

writeFileSync(
  outFile,
  `// Written by nix/web/capabilities.mjs from the OpenAPI document.
import type { components, operations } from "./v1.d.ts";

/** What a session needs to be allowed to call each operation. */
export const OPERATION_CAPABILITY = {
${entries}} as const satisfies Record<keyof operations, components["schemas"]["Capability"]>;
`,
);
