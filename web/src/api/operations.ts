import type { Scope } from "./scope.ts";
import { OPERATION_SCOPE } from "./generated/scopes.ts";
import type { operations, paths } from "./generated/v1.d.ts";

export const API_PREFIX = "/api/v1";

export type OperationId = keyof operations;

type Responses<K extends OperationId> = operations[K]["responses"];
type SuccessCode<K extends OperationId> = Extract<keyof Responses<K>, 200 | 201 | 202 | 204>;

/** How the operation's success response carries its body. */
type ResponseKind<K extends OperationId> = Responses<K>[SuccessCode<K>] extends {
  content: { "application/octet-stream": unknown };
}
  ? "bytes"
  : [Success<K>] extends [undefined]
    ? "empty"
    : "json";

/** The body of the operation's success response; `undefined` if it has none. */
export type Success<K extends OperationId> =
  Responses<K>[SuccessCode<K>] extends { content: { "application/json": infer Body } } ? Body : undefined;

/** The body the operation takes; `never` if it takes none. */
export type RequestBody<K extends OperationId> = operations[K] extends {
  requestBody: { content: { "application/json": infer Body } };
}
  ? Body
  : never;

/** The parameters of the operation's path, by name; `never` if it has none. */
export type PathParams<K extends OperationId> = NonNullable<operations[K]["parameters"]["path"]>;

/**
 * `template`, a path of the API document, with each `{name}` segment replaced by
 * `params[name]`, percent-encoded.
 */
export function expandPath(template: string, params: Readonly<Record<string, string>> = {}): string {
  return template.replace(/\{([^}]+)\}/g, (_match, name: string) => {
    const value = params[name];
    if (value === undefined) throw new Error(`missing path parameter ${name}`);
    return encodeURIComponent(value);
  });
}

type Lowercase_ = "get" | "head" | "post" | "put" | "delete";

/** The path and lowercase method the OpenAPI document gives the operation. */
type RouteOf<K extends OperationId> = {
  [P in keyof paths]: {
    [M in Lowercase_]: [NonNullable<paths[P][M]>] extends [never]
      ? never
      : [NonNullable<paths[P][M]>] extends [operations[K]]
        ? [operations[K]] extends [NonNullable<paths[P][M]>]
          ? [P, M]
          : never
        : never;
  }[Lowercase_];
}[keyof paths];

export interface OperationMeta {
  /** What the session needs to ask for this, as the API declares it. */
  scope: Scope;
  /** Whether the request can be repeated without a further effect. */
  idempotent: boolean;
  /** Overrides the transport's timeout. */
  timeoutMs?: number;
}

/** What the descriptor of an operation has to say, as the API document declares it. */
type Descriptor<K extends OperationId> = OperationMeta & {
  method: Uppercase<RouteOf<K>[1]>;
  path: RouteOf<K>[0];
  response: ResponseKind<K>;
};

/**
 * Every operation of the API. An operation added to the API has no entry here
 * until it is written, and an entry that disagrees with the document's method,
 * path or response doesn't type-check.
 */
export const OPERATIONS = {
  GetSystem: {
    method: "GET",
    path: "/system",
    response: "json",
    idempotent: true,
    scope: OPERATION_SCOPE.GetSystem,
  },
  GetRebootStatus: {
    method: "GET",
    path: "/system/reboot",
    response: "json",
    idempotent: true,
    scope: OPERATION_SCOPE.GetRebootStatus,
  },
  Reboot: {
    method: "POST",
    path: "/system/reboot",
    response: "empty",
    idempotent: false,
    scope: OPERATION_SCOPE.Reboot,
  },
  GetStorage: {
    method: "GET",
    path: "/storage",
    response: "json",
    idempotent: true,
    scope: OPERATION_SCOPE.GetStorage,
  },
  GetLayout: {
    method: "GET",
    path: "/storage/layout",
    response: "json",
    idempotent: true,
    scope: OPERATION_SCOPE.GetLayout,
  },
  GetRegions: {
    method: "GET",
    path: "/storage/regions",
    response: "json",
    idempotent: true,
    scope: OPERATION_SCOPE.GetRegions,
  },
  GetRegion: {
    method: "GET",
    path: "/storage/regions/{region}",
    response: "json",
    idempotent: true,
    scope: OPERATION_SCOPE.GetRegion,
  },
  GetView: {
    method: "GET",
    path: "/storage/regions/{region}/views/{view}",
    response: "bytes",
    idempotent: true,
    scope: OPERATION_SCOPE.GetView,
  },
  GetViewHead: {
    method: "HEAD",
    path: "/storage/regions/{region}/views/{view}",
    response: "empty",
    idempotent: true,
    scope: OPERATION_SCOPE.GetViewHead,
  },
} as const satisfies { [K in OperationId]: Descriptor<K> };

/**
 * The operations whose response the typed client reads: those with a JSON body or
 * none. A binary body and its `HEAD` are fetched by URL ({@link operationUrl}).
 */
export type ClientOperationId = {
  [K in OperationId]: ResponseKind<K> extends "bytes" ? never : RouteOf<K>[1] extends "head" ? never : K;
}[OperationId];

/**
 * The URL of an operation answered with bytes, for a link or a download, with the
 * parameters of its path.
 */
export function operationUrl(
  operation: keyof typeof OPERATIONS,
  path: Readonly<Record<string, string>> = {},
): string {
  const { path: template }: { path: string } = OPERATIONS[operation];
  return `${API_PREFIX}${expandPath(template, path)}`;
}
