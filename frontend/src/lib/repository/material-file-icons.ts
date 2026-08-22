// File and folder associations come from Material Icon Theme's generated VS Code manifest.
type IconThemeManifest = {
  file?: string;
  folder?: string;
  folderExpanded?: string;
  fileNames?: Record<string, string>;
  fileExtensions?: Record<string, string>;
  folderNames?: Record<string, string>;
  folderNamesExpanded?: Record<string, string>;
  iconDefinitions?: Record<string, { iconPath: string }>;
};

declare const __MATERIAL_ICON_THEME__: IconThemeManifest;

const ICON_LOADERS = import.meta.glob<string>(
  "../../../node_modules/material-icon-theme/icons/*.svg",
  {
    import: "default",
    query: "?no-inline",
  },
);

const ICON_PATH_PREFIX = "../../../node_modules/material-icon-theme/icons/";
const iconCache = new Map<string, Promise<string>>();
const missingIcon = Promise.resolve("");

function normalizePath(path: string): string {
  return path
    .replaceAll("\\", "/")
    .replace(/^\/+|\/+$/g, "")
    .toLowerCase();
}

function basename(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function association(
  associations: Record<string, string> | undefined,
  path: string,
): string | undefined {
  if (!associations) return undefined;

  return associations[path] ?? associations[basename(path)];
}

function extensionAssociation(path: string): string | undefined {
  const extensions = __MATERIAL_ICON_THEME__.fileExtensions;
  if (!extensions) return undefined;

  const name = basename(path);
  const segments = name.split(".");
  for (let index = 1; index < segments.length; index += 1) {
    const icon = extensions[segments.slice(index).join(".")];
    if (icon) return icon;
  }

  return undefined;
}

function loadIcon(filename: string): Promise<string> {
  const path = `${ICON_PATH_PREFIX}${filename}`;
  const cached = iconCache.get(path);
  if (cached) return cached;

  const loader = ICON_LOADERS[path];
  if (!loader) return missingIcon;

  const pending = loader();
  iconCache.set(path, pending);
  return pending;
}

function iconUrl(
  icon: string | undefined,
  fallback: string | undefined,
): Promise<string> {
  const definition =
    __MATERIAL_ICON_THEME__.iconDefinitions?.[icon ?? ""] ??
    __MATERIAL_ICON_THEME__.iconDefinitions?.[fallback ?? ""];
  const filename = definition?.iconPath.split("/").pop();
  return filename ? loadIcon(filename) : missingIcon;
}

export function materialFileIcon(path: string): Promise<string> {
  const normalized = normalizePath(path);
  const icon =
    association(__MATERIAL_ICON_THEME__.fileNames, normalized) ??
    extensionAssociation(normalized);

  return iconUrl(icon, __MATERIAL_ICON_THEME__.file);
}

export function materialFolderIcon(
  path: string,
  expanded: boolean,
): Promise<string> {
  const normalized = normalizePath(path);
  const associations = expanded
    ? __MATERIAL_ICON_THEME__.folderNamesExpanded
    : __MATERIAL_ICON_THEME__.folderNames;
  const fallback = expanded
    ? __MATERIAL_ICON_THEME__.folderExpanded
    : __MATERIAL_ICON_THEME__.folder;

  return iconUrl(association(associations, normalized), fallback);
}
