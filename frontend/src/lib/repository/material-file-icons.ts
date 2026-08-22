type MaterialIconManifest = {
  file?: string;
  folder?: string;
  folderExpanded?: string;
  fileNames?: Record<string, string>;
  fileExtensions?: Record<string, string>;
  folderNames?: Record<string, string>;
  folderNamesExpanded?: Record<string, string>;
};

declare const __MATERIAL_ICON_THEME_BASE__: string;

const manifestUrl = `${__MATERIAL_ICON_THEME_BASE__}/manifest.json`;
let manifestRequest: Promise<MaterialIconManifest> | null = null;
const fileIconCache = new Map<string, Promise<string>>();
const folderIconCache = new Map<string, Promise<string>>();

function normalizePath(path: string) {
  return path
    .replaceAll("\\", "/")
    .replace(/^\/+|\/+$/g, "")
    .toLowerCase();
}

function basename(path: string) {
  return path.slice(path.lastIndexOf("/") + 1);
}

function association(
  associations: Record<string, string> | undefined,
  path: string,
) {
  return associations?.[path] ?? associations?.[basename(path)];
}

function extensionAssociation(
  extensions: Record<string, string> | undefined,
  path: string,
) {
  if (!extensions) return undefined;
  const segments = basename(path).split(".");
  for (let index = 1; index < segments.length; index += 1) {
    const icon = extensions[segments.slice(index).join(".")];
    if (icon) return icon;
  }
  return undefined;
}

function iconUrl(filename: string | undefined) {
  return filename
    ? `${__MATERIAL_ICON_THEME_BASE__}/${encodeURIComponent(filename)}`
    : "";
}

function loadManifest() {
  manifestRequest ??= fetch(manifestUrl, { cache: "force-cache" }).then(
    async (response) => {
      if (!response.ok) {
        throw new Error(
          `Could not load Material Icon Theme (${response.status})`,
        );
      }
      return (await response.json()) as MaterialIconManifest;
    },
  );
  return manifestRequest;
}

export function preloadMaterialIconTheme() {
  return loadManifest();
}

export function materialFileIcon(path: string) {
  const normalized = normalizePath(path);
  const cached = fileIconCache.get(normalized);
  if (cached) return cached;

  const pending = loadManifest().then((manifest) => {
    const filename =
      association(manifest.fileNames, normalized) ??
      extensionAssociation(manifest.fileExtensions, normalized) ??
      manifest.file;
    return iconUrl(filename);
  });
  fileIconCache.set(normalized, pending);
  return pending;
}

export function materialFolderIcon(path: string, expanded: boolean) {
  const normalized = normalizePath(path);
  const key = `${expanded ? "open" : "closed"}\0${normalized}`;
  const cached = folderIconCache.get(key);
  if (cached) return cached;

  const pending = loadManifest().then((manifest) => {
    const associations = expanded
      ? manifest.folderNamesExpanded
      : manifest.folderNames;
    const fallback = expanded ? manifest.folderExpanded : manifest.folder;
    return iconUrl(association(associations, normalized) ?? fallback);
  });
  folderIconCache.set(key, pending);
  return pending;
}
