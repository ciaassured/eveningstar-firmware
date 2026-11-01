import type { Scope } from "./scope.ts";
import { OPERATION_SCOPE } from "./generated/scopes.ts";
import type { operations, paths } from "./generated/v1.d.ts";

export const API_PREFIX = "/api/v1";

export type OperationId = keyof operations;

type Responses<K extends OperationId> = operations[K]["responses"];
type SuccessCode<K extends OperationId> = Extract<keyof Responses<K>, 200 | 201 | 202 | 204>;
type SuccessContent<K extends OperationId> = Responses<K>[SuccessCode<K>] extends { content: infer Content }
  ? Content
  : undefined;

/** The body of the operation's success response; `undefined` if it has none or it isn't JSON. */
export type Success<K extends OperationId> =
  SuccessContent<K> extends { "application/json": infer Body } ? Body : undefined;

/** The operations that answer with bytes rather than JSON. */
export type BinaryOperationId = {
  [K in OperationId]: SuccessContent<K> extends { "application/octet-stream": unknown } ? K : never;
}[OperationId];

/** The operations that answer with JSON or nothing. */
export type JsonOperationId = Exclude<OperationId, BinaryOperationId>;

/** The body the operation takes; `never` if it takes none. */
export type RequestBody<K extends OperationId> = operations[K] extends {
  requestBody: { content: { "application/json": infer Body } };
}
  ? Body
  : never;

/** The parameters of the operation's path, such as `{ region }`; `never` if it has none. */
export type PathParams<K extends OperationId> = operations[K]["parameters"] extends { path: infer Params }
  ? Params
  : never;

/** The request headers the operation reads, such as `Range`; `never` if it reads none. */
export type RequestHeaders<K extends OperationId> = NonNullable<operations[K]["parameters"]["header"]>;

type Lowercase_ = "get" | "post" | "put" | "delete";

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
  response: K extends BinaryOperationId ? "binary" : [Success<K>] extends [undefined] ? "empty" : "json";
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
  GetInternalFlash: {
    method: "GET",
    path: "/storage/internal",
    response: "json",
    idempotent: true,
    scope: OPERATION_SCOPE.GetInternalFlash,
  },
  GetRegions: {
    method: "GET",
    path: "/storage/internal/regions",
    response: "json",
    idempotent: true,
    scope: OPERATION_SCOPE.GetRegions,
  },
  GetPartitionTable: {
    method: "GET",
    path: "/storage/internal/table",
    response: "json",
    idempotent: true,
    scope: OPERATION_SCOPE.GetPartitionTable,
  },
  GetRegionContent: {
    method: "GET",
    path: "/storage/internal/regions/{region}/content",
    response: "binary",
    idempotent: true,
    scope: OPERATION_SCOPE.GetRegionContent,
  },
} as const satisfies { [K in OperationId]: Descriptor<K> };

/**
 * The path with each `{name}` replaced by its parameter, encoded as one
 * segment. A parameter the template names and `params` lacks is left as it is,
 * which the device answers with `404`.
 */
export function fillPath(template: string, params: Readonly<Record<string, string>> | undefined): string {
  return template.replace(/\{([^}]+)\}/g, (whole, name: string) => {
    const value = params?.[name];
    return value === undefined ? whole : encodeURIComponent(value);
  });
}
