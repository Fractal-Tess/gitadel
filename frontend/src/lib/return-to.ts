const fallback = "/";
const probeOrigin = "http://return-to.invalid";
const controlCharacter = /[\u0000-\u001f\u007f]/u;

/**
 * Reduces a `returnTo` query value to a path on this origin.
 *
 * The value arrives from the address bar, so anything that a browser could
 * resolve to another origin falls back to the front page: absolute URLs,
 * protocol-relative `//host` forms, backslashes (which browsers read as
 * slashes) and control characters (which browsers strip before resolving).
 */
export function safeReturnTo(value: string | null | undefined): string {
  if (!value || !value.startsWith("/")) return fallback;
  if (value[1] === "/" || value.includes("\\")) return fallback;
  if (controlCharacter.test(value)) return fallback;
  let url: URL;
  try {
    url = new URL(value, probeOrigin);
  } catch {
    return fallback;
  }
  if (url.origin !== probeOrigin) return fallback;
  // Returning to the sign-in page itself would only bounce once more.
  if (url.pathname === "/login" || url.pathname === "/login/") return fallback;
  return value;
}

/**
 * The OAuth authorize endpoint is answered by the server, not by a page of
 * this app, so reaching it takes a document navigation.
 */
export function isServerRoute(path: string): boolean {
  return path.startsWith("/login/oauth/authorize?");
}
