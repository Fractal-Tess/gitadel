import hljs from "highlight.js/lib/common";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import nix from "highlight.js/lib/languages/nix";

hljs.registerLanguage("dockerfile", dockerfile);
hljs.registerLanguage("nix", nix);

const LANGUAGES: Record<string, string> = {
  astro: "xml",
  bash: "bash",
  c: "c",
  cc: "cpp",
  cpp: "cpp",
  cs: "csharp",
  css: "css",
  diff: "diff",
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
  markdown: "markdown",
  md: "markdown",
  mdx: "markdown",
  nix: "nix",
  patch: "diff",
  php: "php",
  pl: "perl",
  pm: "perl",
  py: "python",
  r: "r",
  rb: "ruby",
  rs: "rust",
  sass: "scss",
  scss: "scss",
  sh: "bash",
  sql: "sql",
  svelte: "xml",
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

function languageForPath(path: string): string | null {
  const name = path.split("/").pop()?.toLowerCase() ?? "";
  const extension = name.includes(".") ? (name.split(".").pop() ?? "") : "";
  return FILE_LANGUAGES[name] ?? LANGUAGES[extension] ?? null;
}

export function languageLabel(path: string): string {
  const language = languageForPath(path);
  return language ? (LANGUAGE_LABELS[language] ?? language) : "Plain text";
}

export function highlight(path: string, source: string): string {
  const language = languageForPath(path) ?? "plaintext";
  return hljs.highlight(source, { language, ignoreIllegals: true }).value;
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

export function trustedHtml(html: string): (node: HTMLElement) => void {
  return (node) => {
    node.innerHTML = html;
  };
}
