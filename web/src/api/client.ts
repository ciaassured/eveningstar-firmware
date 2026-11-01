import type { ApiFailure, FailureContext } from "./failure.ts";
import { API_PREFIX, OPERATIONS, fillPath } from "./operations.ts";
import type {
  BinaryOperationId,
  JsonOperationId,
  OperationMeta,
  PathParams,
  RequestBody,
  RequestHeaders,
  Success,
} from "./operations.ts";
import type { DownloadCall, DownloadHead, Priority, Transport } from "./transport.ts";

export type Result<T> =
  | { ok: true; value: T; context: FailureContext }
  | { ok: false; failure: ApiFailure; context: FailureContext };

export interface CallOptions {
  /** Who is asking; by default, a view load. */
  priority?: Priority;
  /** The page this is for: leaving it aborts the request. */
  group?: string;
  /** Overrides whether the transport tries again after a transient failure. */
  retry?: boolean;
}

/** The options for a call with the priority and, if there is one, the page it is for. */
export function callOptions(priority: Priority, group?: string): CallOptions {
  return group === undefined ? { priority } : { priority, group };
}

type WithParams<K extends JsonOperationId | BinaryOperationId> = [PathParams<K>] extends [never]
  ? unknown
  : { params: PathParams<K> };

type Args<K extends JsonOperationId> = [RequestBody<K>] extends [never]
  ? [PathParams<K>] extends [never]
    ? [options?: CallOptions]
    : [options: CallOptions & WithParams<K>]
  : [options: CallOptions & WithParams<K> & { body: RequestBody<K> }];

/** What a download asks for, and where its answer goes. */
export type DownloadOptions<K extends BinaryOperationId> = WithParams<K> & {
  headers?: RequestHeaders<K>;
  /** Overrides how long the download waits for the next bytes. */
  stallMs?: number;
  onHead?: (head: DownloadHead) => void;
  onChunk: (chunk: Uint8Array) => void;
};

/** The device's API, one typed call per operation. */
export interface ApiClient {
  request<K extends JsonOperationId>(operation: K, ...args: Args<K>): Promise<Result<Success<K>>>;
  /** Downloads the body of a binary operation beside the queue; see `Transport.download`. */
  download<K extends BinaryOperationId>(operation: K, options: DownloadOptions<K>): DownloadCall;
}

function pathOf(template: string, options: object | undefined): string {
  const params = options !== undefined && "params" in options ? (options.params as Record<string, string>) : undefined;
  return `${API_PREFIX}${fillPath(template, params)}`;
}

export function createApiClient(transport: Transport): ApiClient {
  return {
    async request<K extends JsonOperationId>(operation: K, ...args: Args<K>): Promise<Result<Success<K>>> {
      const meta = OPERATIONS[operation];
      const options: (CallOptions & { body?: unknown }) | undefined = args[0];
      const priority = options?.priority ?? "view";
      const { timeoutMs }: OperationMeta = meta;
      const outcome = await transport.send({
        method: meta.method,
        path: pathOf(meta.path, options),
        idempotent: meta.idempotent,
        priority,
        resource: operation,
        ...(options?.body === undefined ? {} : { body: options.body }),
        ...(options?.group === undefined ? {} : { group: options.group }),
        ...(options?.retry === undefined ? {} : { retry: options.retry }),
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

    download<K extends BinaryOperationId>(operation: K, options: DownloadOptions<K>): DownloadCall {
      const meta = OPERATIONS[operation];
      const headers: Record<string, string> = {};
      for (const [name, value] of Object.entries(options.headers ?? {})) {
        if (typeof value === "string") headers[name] = value;
      }
      return transport.download({
        path: pathOf(meta.path, options),
        headers,
        onChunk: options.onChunk,
        ...(options.onHead === undefined ? {} : { onHead: options.onHead }),
        ...(options.stallMs === undefined ? {} : { stallMs: options.stallMs }),
      });
    },
  };
}
