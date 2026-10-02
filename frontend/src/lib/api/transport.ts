import { z } from "zod";

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

const errorSchema = z.object({
  error: z.object({
    code: z.string(),
    message: z.string(),
  }),
});

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

async function send(path: string, init: RequestInit): Promise<Response> {
  const headers = new Headers(init.headers);
  headers.set("accept", "application/json");
  if (init.body !== undefined && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  let response: Response;
  try {
    response = await fetch(path, {
      ...init,
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

export async function requestJson<T>(
  path: string,
  schema: z.ZodType<T>,
  init: RequestInit = {},
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

export async function requestEmpty(
  path: string,
  init: RequestInit = {},
): Promise<void> {
  await send(path, init);
}

export function jsonBody(value: unknown): string {
  return JSON.stringify(value);
}

/** The text to show a person for anything a request can throw. */
export function errorMessage(
  caught: unknown,
  fallback = "The request failed.",
): string {
  return caught instanceof Error && caught.message ? caught.message : fallback;
}
