import { assertNever } from "../lib/assert-never.ts";
import type { components } from "./generated/v1.d.ts";

export type ErrorCode = components["schemas"]["ErrorCode"];

/** Why a request has no answer, as far as the client can tell. */
export type ApiFailure =
  | { kind: "network" }
  | { kind: "timeout"; afterMs: number }
  | { kind: "busy"; retryAfterS?: number }
  | { kind: "unauthorized" }
  | { kind: "forbidden" }
  | { kind: "locked"; holder?: string; since?: string }
  | { kind: "api"; status: number; code: ErrorCode | (string & {}) }
  | { kind: "decode"; status: number }
  | { kind: "aborted" }
  | { kind: "unknown-outcome"; cause: ApiFailure };

/** What the client did, for the details a failure can show. */
export interface FailureContext {
  method: string;
  path: string;
  attempts: number;
  /** Milliseconds since the epoch. */
  at: number;
}

export type Action = "retry" | "reload" | "sign-in";

export interface FailureMessage {
  title: string;
  explanation: string;
  actions: readonly Action[];
}

/** Whether trying the same request again later may succeed. */
export function isTransient(failure: ApiFailure): boolean {
  switch (failure.kind) {
    case "network":
    case "timeout":
    case "busy":
      return true;
    case "unknown-outcome":
      return false;
    case "unauthorized":
    case "forbidden":
    case "locked":
    case "api":
    case "decode":
    case "aborted":
      return false;
    default:
      return assertNever(failure);
  }
}

function describeCode(code: string): { title: string; explanation: string } {
  switch (code) {
    case "bad_request":
      return {
        title: "The device rejected the request",
        explanation: "The request wasn't what the device expects. This page and the device may be out of step.",
      };
    case "not_found":
      return {
        title: "The device doesn't have that",
        explanation: "The device has nothing at the address requested. This page and the device may be out of step.",
      };
    case "method_not_allowed":
      return {
        title: "The device doesn't allow that",
        explanation: "The device doesn't accept that kind of request there. This page and the device may be out of step.",
      };
    case "payload_too_large":
      return {
        title: "The request was too large",
        explanation: "The device can't read a request that large.",
      };
    default:
      // A code from a firmware newer than this page.
      return {
        title: "The device reported an error",
        explanation: `The device answered with the error ${code}, which this page doesn't know. Reload the page to get the version that matches the device.`,
      };
  }
}

/** A short title, the likely cause, and what the person can do about it. */
export function describeFailure(failure: ApiFailure): FailureMessage {
  switch (failure.kind) {
    case "network":
      return {
        title: "Can't reach the device",
        explanation:
          "It may be restarting, its network may be down, or it may be serving too many clients.",
        actions: ["retry"],
      };
    case "timeout":
      return {
        title: "The device didn't answer in time",
        explanation: `No answer came within ${Math.round(failure.afterMs / 1_000)} seconds. The device may be busy, or the network may be slow or dropping packets.`,
        actions: ["retry"],
      };
    case "busy":
      return {
        title: "The device is busy",
        explanation:
          failure.retryAfterS === undefined
            ? "It is serving as many clients as it can. Close other tabs or wait a moment."
            : `It is serving as many clients as it can and asked for ${failure.retryAfterS} seconds. Close other tabs or wait a moment.`,
        actions: ["retry"],
      };
    case "unauthorized":
      return {
        title: "Sign in again",
        explanation: "The device doesn't recognise this session, or it has expired.",
        actions: ["sign-in"],
      };
    case "forbidden":
      return {
        title: "Not allowed",
        explanation: "This account doesn't have the access that needs.",
        actions: [],
      };
    case "locked":
      return {
        title: "The configuration is locked",
        explanation:
          failure.holder === undefined
            ? "Someone else is editing the configuration."
            : `${failure.holder} is editing the configuration.`,
        actions: ["retry"],
      };
    case "api": {
      const { title, explanation } = describeCode(failure.code);
      return { title, explanation, actions: ["reload"] };
    }
    case "decode":
      return {
        title: "The device's answer wasn't understood",
        explanation:
          "The answer wasn't in the form this page expects. A proxy or an older page may be in the way.",
        actions: ["reload", "retry"],
      };
    case "aborted":
      return {
        title: "Cancelled",
        explanation: "The request was cancelled because you left the page.",
        actions: [],
      };
    case "unknown-outcome": {
      const cause = describeFailure(failure.cause);
      return {
        title: "The result isn't known",
        explanation: `${cause.title}. The device may or may not have received the request, so it wasn't repeated.`,
        actions: ["retry"],
      };
    }
    default:
      return assertNever(failure);
  }
}

/** The lines of the failure's details: what was asked, and what came back. */
export function failureDetails(failure: ApiFailure, context: FailureContext): string[] {
  const lines = [`${context.method} ${context.path}`];
  switch (failure.kind) {
    case "api":
      lines.push(`status ${failure.status}`, `code ${failure.code}`);
      break;
    case "decode":
      lines.push(`status ${failure.status}`);
      break;
    case "timeout":
      lines.push(`timed out after ${failure.afterMs} ms`);
      break;
    case "busy":
      if (failure.retryAfterS !== undefined) lines.push(`retry after ${failure.retryAfterS} s`);
      break;
    case "unknown-outcome":
      lines.push(...failureDetails(failure.cause, context).slice(1));
      break;
    case "network":
    case "unauthorized":
    case "forbidden":
    case "locked":
    case "aborted":
      break;
    default:
      return assertNever(failure);
  }
  lines.push(`kind ${failure.kind}`, `attempts ${context.attempts}`, new Date(context.at).toISOString());
  return lines;
}
