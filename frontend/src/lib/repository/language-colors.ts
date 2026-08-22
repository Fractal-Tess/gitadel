declare const __LINGUIST_LANGUAGE_COLORS__: Readonly<Record<string, string>>;

export function languageColor(language: string): string {
  const normalized = language.trim().toLowerCase();
  const canonical = __LINGUIST_LANGUAGE_COLORS__[normalized];
  if (canonical) return canonical;

  let hash = 2_166_136_261;
  for (let index = 0; index < normalized.length; index += 1) {
    hash = Math.imul(hash ^ normalized.charCodeAt(index), 16_777_619);
  }

  return `hsl(${(hash >>> 0) % 360} 58% 55%)`;
}
