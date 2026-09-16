export type SubmoduleLinks = {
  repositoryUrl: string | null;
  commitUrl: string | null;
  label: string | null;
  host: string | null;
};

type Provider = "github" | "gitlab" | "bitbucket";

type ParsedRemote = {
  url: URL;
  provider: Provider | null;
};

const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f-\u009f]/;
const INVALID_PERCENT_ESCAPE = /%(?![0-9a-f]{2})/i;
const UNSAFE_ENCODED_PATH = /%(?:2e|2f|5c)/i;
const DOT_PATH_SEGMENT = /(?:^|\/)\.{1,2}(?:\/|$)/;
const COMMIT_OID = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i;

const EMPTY_LINKS: SubmoduleLinks = {
  repositoryUrl: null,
  commitUrl: null,
  label: null,
  host: null,
};

function emptyLinks(): SubmoduleLinks {
  return { ...EMPTY_LINKS };
}

function cleanInput(value: string | null | undefined): string | null {
  if (typeof value !== "string" || CONTROL_CHARACTERS.test(value)) return null;
  const trimmed = value.trim();
  if (!trimmed || INVALID_PERCENT_ESCAPE.test(trimmed)) return null;
  return trimmed;
}

function hasUnsafeTraversal(value: string): boolean {
  // Decode twice so an encoded escape cannot become traversal after another
  // consumer decodes the URL. Malformed escapes are rejected by cleanInput.
  let candidate = value;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    if (UNSAFE_ENCODED_PATH.test(candidate)) return true;
    try {
      const decoded = decodeURIComponent(candidate);
      if (decoded === candidate) break;
      candidate = decoded;
    } catch {
      return true;
    }
  }
  return false;
}

function providerForHost(hostname: string): Provider | null {
  const host = hostname.toLowerCase();
  if (host === "github.com" || host === "www.github.com") return "github";
  if (host === "gitlab.com" || host === "www.gitlab.com") return "gitlab";
  if (host === "bitbucket.org" || host === "www.bitbucket.org") {
    return "bitbucket";
  }
  return null;
}

function parseHttpUrl(value: string): URL | null {
  if (value.startsWith("//") || hasUnsafeTraversal(value)) return null;
  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    return null;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
  if (parsed.username || parsed.password || parsed.search || parsed.hash) {
    return null;
  }
  if (!parsed.hostname || !parsed.pathname || parsed.pathname === "/") {
    return null;
  }
  if (
    CONTROL_CHARACTERS.test(parsed.pathname) ||
    parsed.pathname.includes("\\")
  ) {
    return null;
  }
  return parsed;
}

function parseParent(value: string): URL | null {
  const parent = parseHttpUrl(value);
  if (!parent) return null;
  if (parent.pathname.endsWith("/"))
    parent.pathname = parent.pathname.slice(0, -1);
  return parent;
}

function resolveRelative(value: string, parent: URL): URL | null {
  if (
    value.startsWith("//") ||
    value.includes("\\") ||
    hasUnsafeTraversal(value) ||
    !(
      value === "." ||
      value === ".." ||
      value.startsWith("./") ||
      value.startsWith("../")
    )
  ) {
    return null;
  }

  // Git treats the superproject clone path as a repository directory when it
  // resolves a relative submodule URL. Keeping the trailing slash is what
  // makes ../sibling.git beside /owner/parent.git resolve to /owner/sibling.git.
  const base = new URL(parent.href);
  base.pathname = `${base.pathname.replace(/\/+$/, "")}/`;
  let resolved: URL;
  try {
    resolved = new URL(value, base);
  } catch {
    return null;
  }
  if (resolved.origin !== parent.origin || resolved.search || resolved.hash) {
    return null;
  }
  return resolved;
}

function parseScpRemote(
  value: string,
): { hostname: string; pathname: string } | null {
  const match = /^git@([^/:]+):(.+)$/.exec(value);
  if (!match) return null;
  const [, hostname, path] = match;
  if (
    !hostname ||
    !path ||
    path.startsWith("/") ||
    path.includes("\\") ||
    path.includes("?") ||
    path.includes("#") ||
    CONTROL_CHARACTERS.test(path) ||
    DOT_PATH_SEGMENT.test(path)
  ) {
    return null;
  }
  if (hasUnsafeTraversal(path)) return null;
  return { hostname, pathname: `/${path}` };
}

function parseSshRemote(
  value: string,
): { hostname: string; pathname: string } | null {
  if (value.startsWith("//")) return null;
  if (value.startsWith("ssh://")) {
    if (DOT_PATH_SEGMENT.test(value)) return null;
    let parsed: URL;
    try {
      parsed = new URL(value);
    } catch {
      return null;
    }
    if (
      parsed.protocol !== "ssh:" ||
      parsed.username !== "git" ||
      parsed.password ||
      parsed.search ||
      parsed.hash ||
      !parsed.hostname ||
      !parsed.pathname ||
      parsed.pathname === "/" ||
      parsed.pathname.includes("\\") ||
      hasUnsafeTraversal(parsed.pathname)
    ) {
      return null;
    }
    return { hostname: parsed.hostname, pathname: parsed.pathname };
  }
  return parseScpRemote(value);
}

function normalizeRepositoryUrl(url: URL): URL | null {
  const normalized = new URL(url.href);
  normalized.search = "";
  normalized.hash = "";
  normalized.pathname = normalized.pathname.replace(/\/+$/, "");
  if (!normalized.pathname || normalized.pathname === "/") return null;
  if (/\.git$/i.test(normalized.pathname)) {
    normalized.pathname = normalized.pathname.slice(0, -4);
  }
  if (!normalized.pathname || normalized.pathname === "/") return null;
  return normalized;
}

function labelFor(url: URL): string | null {
  const path = url.pathname.replace(/^\/+|\/+$/g, "");
  if (!path) return null;
  try {
    const label = decodeURIComponent(path);
    return label && !CONTROL_CHARACTERS.test(label) ? label : null;
  } catch {
    return null;
  }
}

function commitUrlForProvider(
  url: URL,
  provider: Provider,
  oid: string,
): string {
  const encodedOid = encodeURIComponent(oid);
  const base = url.toString().replace(/\/$/, "");
  switch (provider) {
    case "github":
      return `${base}/commit/${encodedOid}`;
    case "gitlab":
      return `${base}/-/commit/${encodedOid}`;
    case "bitbucket":
      return `${base}/commits/${encodedOid}`;
  }
}

function commitUrlForGitadel(
  url: URL,
  parent: URL,
  oid: string,
): string | null {
  if (url.origin !== parent.origin) return null;
  const commit = new URL(url.href);
  commit.searchParams.set("view", "commit");
  commit.searchParams.set("oid", oid);
  return commit.toString();
}

export function submoduleLinks(
  rawUrl: string | null | undefined,
  parentCloneUrl: string,
  oid: string,
): SubmoduleLinks {
  const raw = cleanInput(rawUrl);
  const parentInput = cleanInput(parentCloneUrl);
  if (!raw || !parentInput) return emptyLinks();

  const parent = parseParent(parentInput);
  if (!parent) return emptyLinks();

  let parsed: ParsedRemote | null = null;
  const relative = resolveRelative(raw, parent);
  if (relative) {
    parsed = { url: relative, provider: providerForHost(relative.hostname) };
  } else {
    const http = parseHttpUrl(raw);
    if (http) {
      parsed = { url: http, provider: providerForHost(http.hostname) };
    } else {
      const ssh = parseSshRemote(raw);
      if (!ssh) return emptyLinks();
      const provider = providerForHost(ssh.hostname);
      const parentHostMatches =
        ssh.hostname.toLowerCase() === parent.hostname.toLowerCase();
      if (!provider && !parentHostMatches) return emptyLinks();
      const browserUrl = new URL(
        `${parentHostMatches ? parent.protocol : "https:"}//${
          parentHostMatches ? parent.host : ssh.hostname
        }${ssh.pathname}`,
      );

      parsed = { url: browserUrl, provider };
    }
  }
  if (!parsed) return emptyLinks();

  const repository = normalizeRepositoryUrl(parsed.url);
  if (!repository) return emptyLinks();

  const label = labelFor(repository);
  if (!label) return emptyLinks();

  const links: SubmoduleLinks = {
    repositoryUrl: repository.toString(),
    commitUrl: null,
    label,
    host: repository.hostname,
  };
  if (!COMMIT_OID.test(oid)) return links;

  links.commitUrl = parsed.provider
    ? commitUrlForProvider(repository, parsed.provider, oid)
    : commitUrlForGitadel(repository, parent, oid);
  return links;
}
