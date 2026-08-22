const MAX_MARKDOWN_HIGHLIGHT_CHARACTERS = 50_000;

const LANGUAGES: Record<string, string> = {
  astro: "xml",
  bash: "bash",
  console: "bash",
  c: "c",
  cc: "cpp",
  cpp: "cpp",
  cs: "csharp",
  css: "css",
  diff: "diff",
  docker: "dockerfile",
  dockerfile: "dockerfile",
  go: "go",
  gql: "graphql",
  graphql: "graphql",
  h: "c",
  hpp: "cpp",
  html: "xml",
  java: "java",
  js: "javascript",
  jsx: "javascript",
  json: "json",
  json5: "json",
  kt: "kotlin",
  kts: "kotlin",
  less: "less",
  lua: "lua",
  makefile: "makefile",
  markdown: "markdown",
  md: "markdown",
  mdx: "markdown",
  nix: "nix",
  patch: "diff",
  php: "php",
  pl: "perl",
  pm: "perl",
  py: "python",
  plaintext: "plaintext",
  r: "r",
  rb: "ruby",
  rs: "rust",
  sass: "scss",
  scss: "scss",
  sh: "bash",
  shell: "bash",
  "shell-session": "bash",
  sql: "sql",
  svelte: "xml",
  text: "plaintext",
  txt: "plaintext",
  svg: "xml",
  swift: "swift",
  toml: "ini",
  ts: "typescript",
  tsx: "typescript",
  vue: "xml",
  wasm: "wasm",
  xml: "xml",
  yaml: "yaml",
  yml: "yaml",
  zsh: "bash",
};

const FILE_LANGUAGES: Record<string, string> = {
  containerfile: "dockerfile",
  dockerfile: "dockerfile",
  justfile: "makefile",
  makefile: "makefile",
};

const LANGUAGE_LABELS: Record<string, string> = {
  bash: "Shell",
  c: "C",
  cpp: "C++",
  csharp: "C#",
  css: "CSS",
  diff: "Diff",
  dockerfile: "Dockerfile",
  go: "Go",
  graphql: "GraphQL",
  ini: "TOML / INI",
  java: "Java",
  javascript: "JavaScript",
  json: "JSON",
  kotlin: "Kotlin",
  less: "Less",
  lua: "Lua",
  makefile: "Makefile",
  markdown: "Markdown",
  nix: "Nix",
  perl: "Perl",
  php: "PHP",
  python: "Python",
  r: "R",
  ruby: "Ruby",
  rust: "Rust",
  scss: "Sass / SCSS",
  sql: "SQL",
  swift: "Swift",
  typescript: "TypeScript",
  vbnet: "Visual Basic",
  wasm: "WebAssembly",
  xml: "HTML / XML",
  yaml: "YAML",
};

export function formatDate(timestamp: number): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(timestamp * 1000));
}

export function formatSize(size: number | null): string {
  if (size === null) return "";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

function languageForName(name: string): string | null {
  return LANGUAGES[name.toLowerCase()] ?? null;
}

export function languageForPath(path: string): string | null {
  const name = path.split("/").pop()?.toLowerCase() ?? "";
  const extension = name.includes(".") ? (name.split(".").pop() ?? "") : "";
  return languageForName(FILE_LANGUAGES[name] ?? extension);
}

export function languageLabel(path: string): string {
  const language = languageForPath(path);
  return language ? (LANGUAGE_LABELS[language] ?? language) : "Plain text";
}

export function escapeHtml(source: string): string {
  return source
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

export function dayKey(timestamp: number): string {
  const date = new Date(timestamp * 1000);
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
}

export function dayHeading(timestamp: number): string {
  return new Intl.DateTimeFormat(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
    year: "numeric",
  })
    .format(new Date(timestamp * 1000))
    .toUpperCase();
}

export function diffClass(line: string): string {
  if (line.startsWith("+") && !line.startsWith("+++")) {
    return "block min-h-5 whitespace-pre bg-emerald-500/10 px-5 text-emerald-200";
  }
  if (line.startsWith("-") && !line.startsWith("---")) {
    return "block min-h-5 whitespace-pre bg-red-500/10 px-5 text-red-200";
  }
  if (line.startsWith("@@")) {
    return "block min-h-5 whitespace-pre bg-blue-500/10 px-5 font-medium text-blue-300";
  }
  if (line.startsWith("diff ") || line.startsWith("index ")) {
    return "block min-h-5 whitespace-pre px-5 font-semibold text-muted-foreground";
  }
  return "block min-h-5 whitespace-pre px-5 text-foreground/80";
}

export type MarkdownSource = {
  namespace: string;
  name: string;
  revision: string;
  path: string;
};

function resolveRepositoryReference(source: MarkdownSource, reference: string) {
  if (
    !reference ||
    reference.startsWith("#") ||
    reference.startsWith("?") ||
    reference.startsWith("/")
  ) {
    return null;
  }

  try {
    const encodedPath = source.path
      .split("/")
      .map((segment) => encodeURIComponent(segment))
      .join("/");
    const base = new URL(`https://repository.invalid/${encodedPath}`);
    const resolved = new URL(reference, base);
    if (resolved.origin !== base.origin) return null;
    return {
      path: decodeURIComponent(resolved.pathname.slice(1)),
      hash: resolved.hash,
    };
  } catch {
    return null;
  }
}

function rewriteMarkdownReferences(node: HTMLElement, source: MarkdownSource) {
  const api = `/api/v1/repositories/${encodeURIComponent(source.namespace)}/${encodeURIComponent(source.name)}`;
  const repository = `/${encodeURIComponent(source.namespace)}/${encodeURIComponent(source.name)}`;

  for (const image of node.querySelectorAll<HTMLImageElement>("img[src]")) {
    const resolved = resolveRepositoryReference(
      source,
      image.getAttribute("src") ?? "",
    );
    if (!resolved) continue;
    const parameters = new URLSearchParams({
      rev: source.revision,
      path: resolved.path,
    });
    image.src = `${api}/raw?${parameters}`;
  }

  for (const anchor of node.querySelectorAll<HTMLAnchorElement>("a[href]")) {
    const resolved = resolveRepositoryReference(
      source,
      anchor.getAttribute("href") ?? "",
    );
    if (!resolved) continue;
    const parameters = new URLSearchParams({
      rev: source.revision,
      path: resolved.path,
    });
    anchor.href = `${repository}?${parameters}${resolved.hash}`;
  }
}

export function trustedHtml(
  html: string,
  markdownSource?: MarkdownSource,
): (node: HTMLElement) => void {
  return (node) => {
    let cancelled = false;
    node.innerHTML = html;
    if (markdownSource) rewriteMarkdownReferences(node, markdownSource);
    const blocks = Array.from(
      node.querySelectorAll<HTMLElement>("pre code"),
    ).map((code) => ({
      code,
      language: Array.from(code.classList)
        .find((className) => className.startsWith("language-"))
        ?.slice("language-".length),
      source: code.textContent ?? "",
    }));
    for (const { code } of blocks) code.classList.add("hljs");

    const highlightable = blocks.filter(
      ({ language, source }) =>
        language && source.length <= MAX_MARKDOWN_HIGHLIGHT_CHARACTERS,
    );
    if (highlightable.length > 0) {
      void import("$lib/repository/syntax-highlight.js")
        .then(({ highlightLanguage }) => {
          if (cancelled) return;
          for (const { code, language, source } of highlightable) {
            if (!language || !code.isConnected) continue;
            code.innerHTML = highlightLanguage(language, source);
            code.dataset.highlighted = "yes";
          }
        })
        .catch(() => undefined);
    }
    return () => {
      cancelled = true;
    };
  };
}
