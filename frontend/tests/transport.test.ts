import assert from "node:assert/strict";
import { afterEach, describe, test } from "node:test";

import { z } from "zod";

import {
  ApiFailure,
  NETWORK_ERROR,
  errorMessage,
  onUnauthenticated,
  requestEmpty,
  requestJson,
} from "../src/lib/api/transport.js";

const originalFetch = globalThis.fetch;
const originalConsoleError = console.error;
const schema = z.object({ id: z.string() });

function respondWith(body: unknown, status = 200): void {
  globalThis.fetch = (async () =>
    new Response(body === undefined ? null : JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    })) as unknown as typeof fetch;
}

async function failureOf(request: Promise<unknown>): Promise<ApiFailure> {
  try {
    await request;
  } catch (caught) {
    assert.ok(caught instanceof ApiFailure, "expected an ApiFailure");
    return caught;
  }
  throw new Error("The request did not fail.");
}

afterEach(() => {
  globalThis.fetch = originalFetch;
  console.error = originalConsoleError;
});

describe("request error mapping", () => {
  test("returns the validated body of a successful response", async () => {
    respondWith({ id: "a" });
    assert.deepEqual(await requestJson("/api/v1/thing", schema), { id: "a" });
  });

  test("sends JSON headers and same-origin credentials", async () => {
    let seen: RequestInit | undefined;
    globalThis.fetch = (async (_path: string, init: RequestInit) => {
      seen = init;
      return new Response(null, { status: 204 });
    }) as unknown as typeof fetch;
    await requestEmpty("/api/v1/thing", { method: "POST", body: "{}" });
    const headers = new Headers(seen?.headers);
    assert.equal(headers.get("accept"), "application/json");
    assert.equal(headers.get("content-type"), "application/json");
    assert.equal(seen?.credentials, "same-origin");
  });

  test("keeps a content type the caller chose", async () => {
    let seen: RequestInit | undefined;
    globalThis.fetch = (async (_path: string, init: RequestInit) => {
      seen = init;
      return new Response(null, { status: 204 });
    }) as unknown as typeof fetch;
    await requestEmpty("/api/v1/thing", {
      method: "PUT",
      body: "raw",
      headers: { "content-type": "image/png" },
    });
    assert.equal(new Headers(seen?.headers).get("content-type"), "image/png");
  });

  test("uses the server's message and code", async () => {
    respondWith(
      { error: { code: "conflict", message: "Name is taken." } },
      409,
    );
    const failure = await failureOf(requestJson("/api/v1/thing", schema));
    assert.equal(failure.message, "Name is taken.");
    assert.equal(failure.status, 409);
    assert.equal(failure.code, "conflict");
  });

  test("describes a failure that carries no error body", async () => {
    globalThis.fetch = (async () =>
      new Response("<html>Bad gateway</html>", {
        status: 502,
      })) as unknown as typeof fetch;
    for (const request of [
      requestJson("/api/v1/thing", schema),
      requestEmpty("/api/v1/thing"),
    ]) {
      const failure = await failureOf(request);
      assert.equal(failure.message, "Request failed with status 502.");
      assert.equal(failure.code, "request_failed");
    }
  });

  test("maps a rejected fetch to a readable network failure", async () => {
    globalThis.fetch = (async () => {
      throw new TypeError("Failed to fetch");
    }) as unknown as typeof fetch;
    for (const request of [
      requestJson("/api/v1/thing", schema),
      requestEmpty("/api/v1/thing"),
    ]) {
      const failure = await failureOf(request);
      assert.equal(failure.status, 0);
      assert.equal(failure.code, NETWORK_ERROR);
      assert.doesNotMatch(failure.message, /Failed to fetch/u);
    }
  });

  test("lets an aborted request surface as an abort", async () => {
    const controller = new AbortController();
    controller.abort();
    globalThis.fetch = (async () => {
      throw new DOMException("The operation was aborted.", "AbortError");
    }) as unknown as typeof fetch;
    await assert.rejects(
      requestJson("/api/v1/thing", schema, { signal: controller.signal }),
      (caught) =>
        caught instanceof DOMException && caught.name === "AbortError",
    );
  });

  test("reports a body that does not match the schema", async () => {
    console.error = () => undefined;
    respondWith({ id: 4 });
    const failure = await failureOf(requestJson("/api/v1/thing", schema));
    assert.equal(failure.code, "invalid_response");
    assert.equal(failure.status, 200);
  });
});

describe("onUnauthenticated", () => {
  test("is told about a 401 from an ordinary endpoint", async () => {
    let calls = 0;
    const remove = onUnauthenticated(() => calls++);
    try {
      respondWith(
        { error: { code: "unauthorized", message: "Sign in first." } },
        401,
      );
      const failure = await failureOf(requestJson("/api/v1/thing", schema));
      assert.equal(failure.message, "Sign in first.");
      await failureOf(requestEmpty("/api/v1/thing", { method: "DELETE" }));
      assert.equal(calls, 2);
    } finally {
      remove();
    }
  });

  test("ignores the endpoints that establish or describe the session", async () => {
    let calls = 0;
    const remove = onUnauthenticated(() => calls++);
    try {
      respondWith(undefined, 401);
      await failureOf(requestJson("/api/v1/auth/login", schema));
      await failureOf(requestJson("/api/v1/auth/status", schema));
      await failureOf(
        requestJson("/api/v1/auth/passkeys/login/finish", schema),
      );
      assert.equal(calls, 0);
    } finally {
      remove();
    }
  });

  test("ignores other failures and stops once removed", async () => {
    let calls = 0;
    const remove = onUnauthenticated(() => calls++);
    respondWith(undefined, 403);
    await failureOf(requestJson("/api/v1/thing", schema));
    remove();
    respondWith(undefined, 401);
    await failureOf(requestJson("/api/v1/thing", schema));
    assert.equal(calls, 0);
  });
});

describe("errorMessage", () => {
  test("prefers the message of whatever was thrown", () => {
    assert.equal(errorMessage(new ApiFailure("Nope.", 400, "bad")), "Nope.");
    assert.equal(errorMessage(new Error("Boom")), "Boom");
  });

  test("falls back for values that carry no message", () => {
    assert.equal(errorMessage("text"), "The request failed.");
    assert.equal(errorMessage(null, "Sign in failed."), "Sign in failed.");
    assert.equal(errorMessage(new Error(""), "Save failed."), "Save failed.");
  });
});
