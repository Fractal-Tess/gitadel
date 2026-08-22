import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { generateManifest } from 'material-icon-theme';
import * as linguistLanguages from 'linguist-languages';
import type { Language as LinguistLanguage } from 'linguist-languages';
import { defineConfig } from 'vite';

const materialIconManifest = generateManifest({
	activeIconPack: '',
	folders: { theme: 'specific' }
});

function lowercaseAssociations(associations: Record<string, string> | undefined) {
	return associations
		? Object.fromEntries(
				Object.entries(associations).map(([name, icon]) => [name.toLowerCase(), icon])
			)
		: undefined;
}

const linguistLanguageData = linguistLanguages as Record<string, LinguistLanguage>;
const linguistLanguageColors = Object.values(linguistLanguageData).reduce<Record<string, string>>(
	(colors, language) => {
		const color =
			language.color ??
			(language.group ? linguistLanguageData[language.group]?.color : undefined);
		if (!color) return colors;

		colors[language.name.toLowerCase()] = color;
		for (const alias of language.aliases ?? []) colors[alias.toLowerCase()] = color;
		return colors;
	},
	{}
);

const materialIconTheme = {
	file: materialIconManifest.file,
	folder: materialIconManifest.folder,
	folderExpanded: materialIconManifest.folderExpanded,
	fileNames: lowercaseAssociations(materialIconManifest.fileNames),
	fileExtensions: lowercaseAssociations(materialIconManifest.fileExtensions),
	folderNames: lowercaseAssociations(materialIconManifest.folderNames),
	folderNamesExpanded: lowercaseAssociations(materialIconManifest.folderNamesExpanded),
	iconDefinitions: materialIconManifest.iconDefinitions
};

const backendTarget = process.env.GITADEL_DEV_BACKEND ?? 'http://127.0.0.1:8080';

export default defineConfig({
	define: {
		__LINGUIST_LANGUAGE_COLORS__: JSON.stringify(linguistLanguageColors),
		__MATERIAL_ICON_THEME__: JSON.stringify(materialIconTheme)
	},
	plugins: [tailwindcss(), sveltekit()],
	server: {
		allowedHosts: ['kiwi.netbird.cloud'],
		proxy: {
			'/api': backendTarget,
			'/healthz': backendTarget
		}
	}
});
