import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

hljs.registerLanguage("bash", bash);
hljs.registerLanguage("css", css);
hljs.registerLanguage("javascript", javascript);
hljs.registerLanguage("json", json);
hljs.registerLanguage("markdown", markdown);
hljs.registerLanguage("python", python);
hljs.registerLanguage("rust", rust);
hljs.registerLanguage("typescript", typescript);
hljs.registerLanguage("xml", xml);
hljs.registerLanguage("yaml", yaml);

const LANGUAGES: Record<string, string> = {
  rs: "rust",
  sh: "bash",
  bash: "bash",
  css: "css",
  js: "javascript",
  jsx: "javascript",
  ts: "typescript",
  tsx: "typescript",
  json: "json",
  md: "markdown",
  markdown: "markdown",
  py: "python",
  html: "xml",
  xml: "xml",
  svg: "xml",
  yml: "yaml",
  yaml: "yaml",
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

export function highlight(path: string, source: string): string {
  const extension = path.split(".").pop()?.toLowerCase() ?? "";
  const language = LANGUAGES[extension];
  return language
    ? hljs.highlight(source, { language, ignoreIllegals: true }).value
    : hljs.highlightAuto(source).value;
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
