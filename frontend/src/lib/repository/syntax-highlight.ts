import hljs from "highlight.js/lib/common";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import nix from "highlight.js/lib/languages/nix";

import { languageForPath } from "$lib/repository/format.js";

hljs.registerLanguage("dockerfile", dockerfile);
hljs.registerLanguage("nix", nix);

export function highlightLanguage(language: string, source: string) {
  const supported = hljs.getLanguage(language) ? language : "plaintext";
  return hljs.highlight(source, {
    language: supported,
    ignoreIllegals: true,
  }).value;
}

export function highlight(path: string, source: string) {
  return highlightLanguage(languageForPath(path) ?? "plaintext", source);
}
