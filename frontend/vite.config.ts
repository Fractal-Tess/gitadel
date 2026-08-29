import tailwindcss from "@tailwindcss/vite";
import { sveltekit } from "@sveltejs/kit/vite";
import { readFileSync, readdirSync } from "node:fs";
import { basename, join } from "node:path";
import { fileURLToPath, URL } from "node:url";
import * as linguistLanguages from "linguist-languages";
import type { Language as LinguistLanguage } from "linguist-languages";
import { defineConfig, type Plugin } from "vite";

const MIN_LANGUAGE_COLOR_CONTRAST = 3;
const DARK_SURFACE_LUMINANCE = 0.18 ** 3;
const LANGUAGE_COLOR_OVERRIDES: Readonly<Record<string, string>> = {
  JSON: "#f1e05a",
  "JSON with Comments": "#f1e05a",
};

function relativeLuminance(channels: readonly number[]): number {
  const [red = 0, green = 0, blue = 0] = channels.map((channel) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return red * 0.2126 + green * 0.7152 + blue * 0.0722;
}

function darkThemeLanguageColor(color: string): string {
  const channels = [1, 3, 5].map((offset) =>
    Number.parseInt(color.slice(offset, offset + 2), 16),
  );
  const minimumLuminance =
    MIN_LANGUAGE_COLOR_CONTRAST * (DARK_SURFACE_LUMINANCE + 0.05) - 0.05;
  if (relativeLuminance(channels) >= minimumLuminance) return color;

  let lowerMix = 0;
  let upperMix = 1;
  for (let iteration = 0; iteration < 12; iteration += 1) {
    const mix = (lowerMix + upperMix) / 2;
    const mixed = channels.map((channel) => channel + (255 - channel) * mix);
    if (relativeLuminance(mixed) >= minimumLuminance) upperMix = mix;
    else lowerMix = mix;
  }

  const adjusted = channels
    .map((channel) =>
      Math.ceil(channel + (255 - channel) * upperMix)
        .toString(16)
        .padStart(2, "0"),
    )
    .join("");
  return `light-dark(${color}, #${adjusted})`;
}

const linguistLanguageData = linguistLanguages as Record<
  string,
  LinguistLanguage
>;
const linguistLanguageColors = Object.values(linguistLanguageData).reduce<
  Record<string, string>
>((colors, language) => {
  const color =
    LANGUAGE_COLOR_OVERRIDES[language.name] ??
    language.color ??
    (language.group ? linguistLanguageData[language.group]?.color : undefined);
  if (!color) return colors;

  const adaptiveColor = darkThemeLanguageColor(color);
  colors[language.name.toLowerCase()] = adaptiveColor;
  for (const alias of language.aliases ?? [])
    colors[alias.toLowerCase()] = adaptiveColor;
  return colors;
}, {});

type RawMaterialIconManifest = {
  file?: string;
  folder?: string;
  folderExpanded?: string;
  fileNames?: Record<string, string>;
  fileExtensions?: Record<string, string>;
  folderNames?: Record<string, string>;
  folderNamesExpanded?: Record<string, string>;
  iconDefinitions?: Record<string, { iconPath: string }>;
};

const materialIconThemeRoot = fileURLToPath(
  new URL("./node_modules/material-icon-theme/", import.meta.url),
);
const materialIconDirectory = join(materialIconThemeRoot, "icons");
const materialIconPackage = JSON.parse(
  readFileSync(join(materialIconThemeRoot, "package.json"), "utf8"),
) as { version: string };
const materialIconBase = `/_app/immutable/material-icon-theme/${materialIconPackage.version}`;
const materialIconOutputBase = materialIconBase.slice(1);
const materialIconFiles = readdirSync(materialIconDirectory).filter((file) =>
  file.endsWith(".svg"),
);
const materialIconFileSet = new Set(materialIconFiles);
const rawMaterialIconManifest = JSON.parse(
  readFileSync(join(materialIconThemeRoot, "dist/material-icons.json"), "utf8"),
) as RawMaterialIconManifest;

function materialIconFilename(icon: string | undefined) {
  const iconPath =
    rawMaterialIconManifest.iconDefinitions?.[icon ?? ""]?.iconPath;
  return iconPath ? basename(iconPath) : undefined;
}

function materialIconAssociations(
  associations: Record<string, string> | undefined,
) {
  return Object.fromEntries(
    Object.entries(associations ?? {}).flatMap(([name, icon]) => {
      const filename = materialIconFilename(icon);
      return filename ? [[name, filename]] : [];
    }),
  );
}

const materialIconManifest = JSON.stringify({
  file: materialIconFilename(rawMaterialIconManifest.file),
  folder: materialIconFilename(rawMaterialIconManifest.folder),
  folderExpanded: materialIconFilename(rawMaterialIconManifest.folderExpanded),
  fileNames: materialIconAssociations(rawMaterialIconManifest.fileNames),
  fileExtensions: materialIconAssociations(
    rawMaterialIconManifest.fileExtensions,
  ),
  folderNames: materialIconAssociations(rawMaterialIconManifest.folderNames),
  folderNamesExpanded: materialIconAssociations(
    rawMaterialIconManifest.folderNamesExpanded,
  ),
});

function materialIconThemeAssets(): Plugin {
  let emitAssets = false;
  return {
    name: "gitadel-material-icon-theme-assets",
    configResolved(config) {
      emitAssets = config.command === "build" && !config.build.ssr;
    },
    buildStart() {
      if (!emitAssets) return;
      this.emitFile({
        type: "asset",
        fileName: `${materialIconOutputBase}/manifest.json`,
        source: materialIconManifest,
      });
      for (const filename of materialIconFiles) {
        this.emitFile({
          type: "asset",
          fileName: `${materialIconOutputBase}/${filename}`,
          source: readFileSync(join(materialIconDirectory, filename)),
        });
      }
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const pathname = request.url?.split("?", 1)[0];
        if (!pathname?.startsWith(`${materialIconBase}/`)) {
          next();
          return;
        }

        const requested = decodeURIComponent(
          pathname.slice(materialIconBase.length + 1),
        );
        response.setHeader("Cache-Control", "no-cache");
        if (requested === "manifest.json") {
          response.setHeader("Content-Type", "application/json; charset=utf-8");
          response.end(materialIconManifest);
          return;
        }
        if (
          requested !== basename(requested) ||
          !materialIconFileSet.has(requested)
        ) {
          response.statusCode = 404;
          response.end();
          return;
        }
        response.setHeader("Content-Type", "image/svg+xml");
        response.end(readFileSync(join(materialIconDirectory, requested)));
      });
    },
  };
}

const backendTarget =
  process.env.GITADEL_DEV_BACKEND ?? "http://127.0.0.1:8080";

export default defineConfig({
  define: {
    __LINGUIST_LANGUAGE_COLORS__: JSON.stringify(linguistLanguageColors),
    __MATERIAL_ICON_THEME_BASE__: JSON.stringify(materialIconBase),
  },
  plugins: [materialIconThemeAssets(), tailwindcss(), sveltekit()],
  resolve: {
    alias: {
      "@pierre/diffs/web-components": fileURLToPath(
        new URL(
          "./node_modules/@pierre/diffs/dist/components/web-components.js",
          import.meta.url,
        ),
      ),
    },
  },
  server: {
    allowedHosts: ["kiwi.netbird.cloud"],
    proxy: {
      "/api": backendTarget,
      "/healthz": backendTarget,
    },
  },
});
