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
/** Whether a location hash addresses source lines rather than a heading. */
export function isLineHash(hash: string): boolean {
  return /^#L\d+(?:-L?\d+)?$/.test(hash);
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
/**
 * A link pinned to one commit. Source links carry `plain=1` so a rendered
 * document opens on its source; a rendered view may carry a heading hash.
 */
export function codePermalink(
  origin: string,
  namespace: string,
  repository: string,
  commit: string,
  path: string,
  range: LineRange | null,
  options: { plain?: boolean; hash?: string } = {},
): string {
  const url = new URL(
    `/${encodeURIComponent(namespace)}/${encodeURIComponent(repository)}`,
    origin,
  );
  const parameters = new URLSearchParams({ rev: commit, path });
  if (options.plain ?? true) parameters.set("plain", "1");
  url.search = parameters.toString();
  url.hash = range ? rangeHash(range) : (options.hash ?? "");
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
export const fontSizeLimits = { min: 10, max: 24 } as const;
export const tabSizes = [2, 4, 8] as const;

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

const ENTITY = /^&(?:#(\d+)|#x([\da-f]+)|(amp|lt|gt|quot|apos|nbsp));/i;
const NAMED: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: "\u00a0",
};

/**
 * Wraps case-insensitive literal matches of `query` in `<mark>` inside one
 * line of highlighted markup, keeping its syntax spans. A match that crosses a
 * span boundary is closed before the tag and reopened after it, so the output
 * stays balanced. `html` must be escaped markup such as highlight.js emits.
 */
export function markMatches(html: string, query: string): string {
  if (!query) return html;
  // Each unit is one tag, or one decoded text character with its source form.
  const units: Array<{ tag: string } | { text: string; raw: string }> = [];
  let plain = "";
  const starts: number[] = [];
  for (let index = 0; index < html.length; ) {
    if (html[index] === "<") {
      const close = html.indexOf(">", index);
      const end = close === -1 ? html.length : close + 1;
      units.push({ tag: html.slice(index, end) });
      index = end;
      continue;
    }
    let raw = html[index];
    let text = raw;
    if (raw === "&") {
      const entity = ENTITY.exec(html.slice(index, index + 12));
      if (entity) {
        raw = entity[0];
        text = entity[3]
          ? NAMED[entity[3].toLowerCase()]
          : String.fromCodePoint(
              Number.parseInt(entity[1] ?? entity[2], entity[1] ? 10 : 16),
            );
      }
    }
    starts.push(units.length);
    units.push({ text, raw });
    plain += text;
    // Astral characters occupy two code units of `plain`.
    if (text.length === 2) starts.push(units.length - 1);
    index += raw.length;
  }
  const pattern = new RegExp(
    query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
    "gi",
  );
  const marked = new Set<number>();
  for (const match of plain.matchAll(pattern))
    for (let offset = 0; offset < match[0].length; offset += 1)
      marked.add(starts[match.index + offset]);
  if (!marked.size) return html;
  let output = "";
  let open = false;
  for (const [index, unit] of units.entries()) {
    const inside = marked.has(index);
    if ("tag" in unit) {
      if (open) output += "</mark>";
      output += unit.tag;
      open = false;
      continue;
    }
    if (inside && !open) output += "<mark>";
    if (!inside && open) output += "</mark>";
    open = inside;
    output += unit.raw;
  }
  return open ? `${output}</mark>` : output;
}

/** Lines, 1-based, whose commit differs from the line above in blame. */
export function blameHunkStarts(
  lines: Array<{ line: number; commit_oid: string }>,
): Set<number> {
  const starts = new Set<number>();
  let previous = "";
  for (const line of [...lines].sort((a, b) => a.line - b.line)) {
    if (line.commit_oid !== previous) starts.add(line.line);
    previous = line.commit_oid;
  }
  return starts;
}

/** A compact age such as "5m", "3d", "2mo", or "4y" for the blame gutter. */
export function compactAge(timestamp: number, now = Date.now() / 1000): string {
  const seconds = Math.max(0, now - timestamp);
  const steps: Array<[number, string]> = [
    [31_536_000, "y"],
    [2_592_000, "mo"],
    [604_800, "w"],
    [86_400, "d"],
    [3_600, "h"],
    [60, "m"],
  ];
  for (const [size, unit] of steps)
    if (seconds >= size) return `${Math.floor(seconds / size)}${unit}`;
  return "now";
}

/**
 * GitHub-compatible heading slugs: lower case, punctuation dropped, spaces as
 * hyphens, and repeated headings numbered so each anchor stays unique.
 */
export function headingSlugger(): (text: string) => string {
  const seen = new Map<string, number>();
  const used = new Set<string>();
  return (text) => {
    const base = text
      .trim()
      .toLowerCase()
      .replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, "")
      .replace(/\s/g, "-");
    let count = seen.get(base) ?? 0;
    let slug = count ? `${base}-${count}` : base;
    while (used.has(slug)) slug = `${base}-${++count}`;
    seen.set(base, count + 1);
    used.add(slug);
    return slug;
  };
}
