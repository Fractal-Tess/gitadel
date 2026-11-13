import { escapeHtml } from "./escape-html.js";

export type LineRange = { start: number; end: number };
export type ReadingPreferences = {
  wrap: boolean;
  fontSize: number;
  tabSize: number;
};
export const defaultReadingPreferences: ReadingPreferences = {
  wrap: false,
  fontSize: 12,
  tabSize: 4,
};
export const readingPreferencesKey = "gitadel:code-reading-preferences";

export function sourceLines(source: string): string[] {
  if (!source) return [];
  const lines = source.split(/\r?\n/);
  if (lines.at(-1) === "") lines.pop();
  return lines;
}

export function parseLineRange(value: string, count: number): LineRange | null {
  const match = /^(?:#?L)?(\d+)(?:-(?:L)?(\d+))?$/.exec(value.trim());
  if (!match) return null;
  const start = Number(match[1]);
  const end = Number(match[2] ?? match[1]);
  return Number.isSafeInteger(start) &&
    Number.isSafeInteger(end) &&
    start > 0 &&
    end >= start &&
    end <= count
    ? { start, end }
    : null;
}
export function rangeHash(range: LineRange): string {
  return range.start === range.end
    ? `#L${range.start}`
    : `#L${range.start}-L${range.end}`;
}

/** Keep multiline highlight.js spans balanced within each numbered row. */
export function highlightedLines(html: string, count: number): string[] {
  const open: string[] = [];
  const lines: string[] = [];
  let current = "";
  for (const part of html.split(/(<span\b[^>]*>|<\/span>|\r?\n)/)) {
    if (part === "\n" || part === "\r\n") {
      lines.push(current + "</span>".repeat(open.length));
      current = open.join("");
    } else {
      if (part.startsWith("<span")) open.push(part);
      else if (part === "</span>") open.pop();
      current += part;
    }
  }
  if (lines.length < count) lines.push(current);
  return lines.slice(0, count);
}

export function searchMarkup(text: string, query: string): string {
  if (!query) return escapeHtml(text);
  const pattern = new RegExp(
    query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
    "gi",
  );
  let html = "";
  let offset = 0;
  for (const match of text.matchAll(pattern)) {
    html +=
      escapeHtml(text.slice(offset, match.index)) +
      `<mark>${escapeHtml(match[0])}</mark>`;
    offset = match.index + match[0].length;
  }
  return html + escapeHtml(text.slice(offset));
}
export function codePermalink(
  origin: string,
  namespace: string,
  repository: string,
  commit: string,
  path: string,
  range: LineRange | null,
): string {
  const url = new URL(
    `/${encodeURIComponent(namespace)}/${encodeURIComponent(repository)}`,
    origin,
  );
  url.search = new URLSearchParams({
    rev: commit,
    path,
    plain: "1",
  }).toString();
  url.hash = range ? rangeHash(range) : "";
  return url.toString();
}
export function markdownSnippet(
  code: string,
  language: string,
  url: string,
): string {
  let longest = 2;
  for (const match of code.matchAll(/`+/g))
    longest = Math.max(longest, match[0].length);
  const fence = "`".repeat(longest + 1);
  return `[View source](${url})\n\n${fence}${/^[\w+-]+$/.test(language) ? language : ""}\n${code}\n${fence}`;
}
export function readPreferences(value: string | null): ReadingPreferences {
  try {
    const parsed = JSON.parse(value ?? "null");
    return {
      wrap: parsed?.wrap === true,
      fontSize:
        typeof parsed?.fontSize === "number"
          ? Math.min(24, Math.max(10, parsed.fontSize))
          : 12,
      tabSize:
        typeof parsed?.tabSize === "number"
          ? Math.min(8, Math.max(2, Math.round(parsed.tabSize)))
          : 4,
    };
  } catch {
    return { ...defaultReadingPreferences };
  }
}
