/** Client-side checks that mirror the server's rules for website links. */

/** Adds `https://` to an address typed without a scheme, such as `docs.rs`. */
export function withWebsiteScheme(value: string): string {
  const trimmed = value.trim();
  const hostWithPort = /^(?:[a-z\d.-]+|\[[\da-f:]+\]):\d+(?:[/?#]|$)/i.test(trimmed);
  if (!trimmed || (!hostWithPort && /^[a-z][a-z\d+.-]*:/i.test(trimmed))) return trimmed;
  return `https://${trimmed.replace(/^\/+/, "")}`;
}

/** The host a link points at, or "" when it is not a usable address. */
export function websiteHost(value: string): string {
  try {
    const url = new URL(withWebsiteScheme(value));
    return /^https?:$/.test(url.protocol) ? url.hostname : "";
  } catch {
    return "";
  }
}

function normalized(value: string): string {
  try {
    return new URL(withWebsiteScheme(value)).href;
  } catch {
    return withWebsiteScheme(value);
  }
}

/**
 * Why a link cannot be saved, or "" when it can. A row with neither a URL nor
 * a name is blank and is dropped on save instead.
 */
export function websiteProblem(
  url: string,
  label: string,
  earlier: string[],
): string {
  if (!url.trim()) return label.trim() ? "Add a URL or remove this link." : "";
  let parsed: URL;
  try {
    parsed = new URL(withWebsiteScheme(url));
  } catch {
    return "Enter a web address such as https://example.com.";
  }
  if (!/^https?:$/.test(parsed.protocol) || !parsed.hostname)
    return "Use an http:// or https:// address.";
  if (parsed.username || parsed.password)
    return "Remove the username or password from the address.";
  if (earlier.some((other) => other.trim() && normalized(other) === parsed.href))
    return "This link is already in the list.";
  return "";
}
