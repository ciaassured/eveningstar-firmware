import type { ApiFailure, FailureContext } from "./failure.ts";
import { API_PREFIX, OPERATIONS } from "./operations.ts";
import type { OperationId, OperationMeta, RequestBody, Success } from "./operations.ts";
import type { Priority, Transport } from "./transport.ts";

export type Result<T> =
  | { ok: true; value: T; context: FailureContext }
  | { ok: false; failure: ApiFailure; context: FailureContext };

export interface CallOptions {
  /** Who is asking; by default, a view load. */
  priority?: Priority;
  /** The page this is for: leaving it aborts the request. */
  group?: string;
}

/** The options for a call with the priority and, if there is one, the page it is for. */
export function callOptions(priority: Priority, group?: string): CallOptions {
  return group === undefined ? { priority } : { priority, group };
}

type Args<K extends OperationId> = [RequestBody<K>] extends [never]
  ? [options?: CallOptions]
  : [options: CallOptions & { body: RequestBody<K> }];

/** The device's API, one typed call per operation. */
export interface ApiClient {
  request<K extends OperationId>(operation: K, ...args: Args<K>): Promise<Result<Success<K>>>;
}

export function createApiClient(transport: Transport): ApiClient {
  return {
    async request<K extends OperationId>(operation: K, ...args: Args<K>): Promise<Result<Success<K>>> {
      const meta = OPERATIONS[operation];
      const options: (CallOptions & { body?: unknown }) | undefined = args[0];
      const priority = options?.priority ?? "view";
      const { timeoutMs }: OperationMeta = meta;
      const outcome = await transport.send({
        method: meta.method,
        path: `${API_PREFIX}${meta.path}`,
        idempotent: meta.idempotent,
        priority,
        resource: operation,
        ...(options?.body === undefined ? {} : { body: options.body }),
        ...(options?.group === undefined ? {} : { group: options.group }),
        ...(timeoutMs === undefined ? {} : { timeoutMs }),
      });
      if (!outcome.ok) return outcome;

      const { status, body } = outcome.reply;
      const expectsBody = meta.response === "json";
      if (expectsBody !== (body !== undefined)) {
        return { ok: false, failure: { kind: "decode", status }, context: outcome.context };
      }
      return { ok: true, value: body as Success<K>, context: outcome.context };
    },
  };
}
