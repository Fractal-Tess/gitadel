import { z } from "zod";
import type { ContractArgs, ContractMethod, ContractPath, ContractSchema, JsonBody, ApiQuery, ApiSchema } from './contract.js';
import type { ErrorEnvelope } from './generated/types.gen.js';

type TransportInit = RequestInit & { query?: unknown };

export class ApiFailure extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
  ) {
    super(message);
  }
}

/** `ApiFailure.code` of a request that never reached the server. */
export const NETWORK_ERROR = "network_error";

const errorShape = z.object({
  error: z.object({
    code: z.string(),
    message: z.string(),
  }),
});
const errorSchema: ApiSchema<ErrorEnvelope, typeof errorShape> = errorShape;

let unauthenticatedHandler: (() => void) | null = null;

/**
 * Registers the one listener told when a request is refused for lack of a
 * session. Returns a function that removes it again.
 */
export function onUnauthenticated(handler: () => void): () => void {
  unauthenticatedHandler = handler;
  return () => {
    if (unauthenticatedHandler === handler) unauthenticatedHandler = null;
  };
}

// A 401 from these is an answer about the credentials just presented, not
// the news that an established session has ended.
function describesSessionItself(path: string): boolean {
  return path.startsWith("/api/v1/auth/");
}

function isAbort(caught: unknown): boolean {
  return caught instanceof DOMException && caught.name === "AbortError";
}

async function send(path: string, init: TransportInit): Promise<Response> {
  const { query, ...httpInit } = init;
  const parameters = new URLSearchParams();
  if (query !== undefined && (query === null || typeof query !== 'object')) throw new TypeError('API query must be an object');
  for (const [name, value] of Object.entries(query ?? {})) {
    if (value === undefined || value === null) continue;
    for (const item of Array.isArray(value) ? value : [value]) parameters.append(name, String(item));
  }
  const requestPath = parameters.size ? `${path}${path.includes('?') ? '&' : '?'}${parameters}` : path;
  const headers = new Headers(init.headers);
  headers.set("accept", "application/json");
  if (init.body !== undefined && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  let response: Response;
  try {
    response = await fetch(requestPath, {
      ...httpInit,
      headers,
      credentials: "same-origin",
    });
  } catch (caught) {
    // Callers that cancel a request tell it apart by this exact error.
    if (isAbort(caught) || init.signal?.aborted) throw caught;
    throw new ApiFailure(
      "Could not reach the server. Check your connection and try again.",
      0,
      NETWORK_ERROR,
    );
  }
  if (response.ok) return response;

  const payload: unknown = await response.json().catch(() => null);
  const parsed = errorSchema.safeParse(payload);
  if (response.status === 401 && !describesSessionItself(path)) {
    unauthenticatedHandler?.();
  }
  throw new ApiFailure(
    parsed.success
      ? parsed.data.error.message
      : `Request failed with status ${response.status}.`,
    response.status,
    parsed.success ? parsed.data.error.code : "request_failed",
  );
}

export function requestJson<const Url extends string, S extends z.ZodType, const M extends ContractMethod = 'GET'>(
  path: Url,
  schema: ContractSchema<NoInfer<Url>, NoInfer<M>, S>,
  ...options: ContractArgs<NoInfer<Url>, M>
): Promise<z.output<S>>;
export async function requestJson<T>(
  path: string,
  schema: z.ZodType<T>,
  init: TransportInit = {},
): Promise<T> {
  const response = await send(path, init);
  const payload: unknown = await response.json().catch(() => null);
  const parsed = schema.safeParse(payload);
  if (!parsed.success) {
    if (import.meta.env?.DEV) {
      console.error(
        `Invalid response from ${init.method ?? "GET"} ${path}`,
        parsed.error.issues.map((issue) => ({
          path: issue.path.join("."),
          code: issue.code,
          message: issue.message,
        })),
      );
    }
    throw new ApiFailure(
      "The server returned an invalid response.",
      response.status,
      "invalid_response",
    );
  }
  return parsed.data;
}

export function requestEmpty<const Url extends string, const M extends ContractMethod = 'GET'>(
  path: ContractPath<Url, M>,
  ...options: ContractArgs<NoInfer<Url>, M>
): Promise<void>;
export async function requestEmpty(
  path: string,
  init: TransportInit = {},
): Promise<void> {
  await send(path, init);
}

export function jsonBody<T>(value: T): JsonBody<T> {
  return JSON.stringify(value) as JsonBody<T>;
}

/** Keeps query keys as well as values tied to the generated operation DTO. */
export function apiQuery<T extends object>(value: T): ApiQuery<T> {
  return value as ApiQuery<T>;
}

/** The text to show a person for anything a request can throw. */
export function errorMessage(
  caught: unknown,
  fallback = "The request failed.",
): string {
  return caught instanceof Error && caught.message ? caught.message : fallback;
}
