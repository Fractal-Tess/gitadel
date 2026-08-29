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

const errorSchema = z.object({
  error: z.object({
    code: z.string(),
    message: z.string(),
  }),
});

export async function requestJson<T>(
  path: string,
  schema: z.ZodType<T>,
  init: RequestInit = {},
): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set("accept", "application/json");
  if (init.body !== undefined && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const response = await fetch(path, {
    ...init,
    headers,
    credentials: "same-origin",
  });
  const payload: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const parsed = errorSchema.safeParse(payload);
    throw new ApiFailure(
      parsed.success
        ? parsed.data.error.message
        : `Request failed with status ${response.status}.`,
      response.status,
      parsed.success ? parsed.data.error.code : "request_failed",
    );
  }
  const parsed = schema.safeParse(payload);
  if (!parsed.success) {
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
  const headers = new Headers(init.headers);
  headers.set("accept", "application/json");
  if (init.body !== undefined && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const response = await fetch(path, {
    ...init,
    headers,
    credentials: "same-origin",
  });
  if (!response.ok) {
    const payload: unknown = await response.json().catch(() => null);
    const parsed = errorSchema.safeParse(payload);
    throw new ApiFailure(
      parsed.success
        ? parsed.data.error.message
        : `Request failed with status ${response.status}.`,
      response.status,
      parsed.success ? parsed.data.error.code : "request_failed",
    );
  }
}

export function jsonBody(value: unknown): string {
  return JSON.stringify(value);
}
