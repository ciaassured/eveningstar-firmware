import type { Scope } from "./scope.ts";
import { OPERATION_SCOPE } from "./generated/scopes.ts";
import type { operations, paths } from "./generated/v1.d.ts";

export const API_PREFIX = "/api/v1";

export type OperationId = keyof operations;

type Responses<K extends OperationId> = operations[K]["responses"];
type SuccessCode<K extends OperationId> = Extract<keyof Responses<K>, 200 | 201 | 202 | 204>;

/** The body of the operation's success response; `undefined` if it has none. */
export type Success<K extends OperationId> =
  Responses<K>[SuccessCode<K>] extends { content: { "application/json": infer Body } } ? Body : undefined;

/** The body the operation takes; `never` if it takes none. */
export type RequestBody<K extends OperationId> = operations[K] extends {
  requestBody: { content: { "application/json": infer Body } };
}
  ? Body
  : never;

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
  response: [Success<K>] extends [undefined] ? "empty" : "json";
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
} as const satisfies { [K in OperationId]: Descriptor<K> };
