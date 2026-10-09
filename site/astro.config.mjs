// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

const repo = 'https://github.com/nomunomu0504/sub-screen-player';

// Set by the deploy workflow from the SITE_DOMAIN repository variable. With a custom domain
// the site lives at the root; on github.io it would live under /<repository>.
const site = process.env.SITE_URL ?? 'https://subscreen.dev';
const base = process.env.SITE_BASE ?? '/';

export default defineConfig({
	site,
	base,
	integrations: [
		starlight({
			title: 'sub-screen-player',
			description:
				'Drive small USB sub-displays from macOS, Linux and Windows: a clock, images and live frames at up to 60 fps.',
			logo: { src: './src/assets/logo.svg', replacesTitle: false },
			favicon: '/favicon.svg',
			defaultLocale: 'root',
			locales: {
				root: { label: 'English', lang: 'en' },
				ja: { label: '日本語', lang: 'ja' },
			},
			social: [{ icon: 'github', label: 'GitHub', href: repo }],
			// The look of the landing page (src/styles/tokens.css): fonts, colors, headings, asides.
			customCss: ['./src/styles/starlight.css'],
			head: [
				{ tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.googleapis.com' } },
				{ tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: true } },
				{
					tag: 'link',
					attrs: {
						rel: 'stylesheet',
						href: 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;700&family=Noto+Sans+JP:wght@400;500;700&display=swap',
					},
				},
			],
			// Terminals and code stay dark in both themes, like the landing page's.
			expressiveCode: {
				themes: ['starlight-dark'],
				styleOverrides: {
					borderRadius: '12px',
					borderColor: '#1c2431',
					codeFontFamily: "'JetBrains Mono', ui-monospace, Menlo, Consolas, monospace",
					codeBackground: '#0b0e14',
					frames: {
						editorBackground: '#0b0e14',
						editorTabBarBackground: '#10151e',
						editorActiveTabBackground: '#0b0e14',
						terminalBackground: '#0b0e14',
						terminalTitlebarBackground: '#10151e',
						frameBoxShadowCssValue: 'none',
					},
				},
			},
			lastUpdated: true,
			sidebar: [
				{
					label: 'Start here',
					translations: { ja: '使い始める' },
					items: ['getting-started', 'download'],
				},
				{
					label: 'Guides',
					translations: { ja: 'ガイド' },
					items: ['cli', 'api'],
				},
				{
					label: 'Displays',
					translations: { ja: '対応ディスプレイ' },
					items: ['devices/d92'],
				},
				{
					label: 'Development',
					translations: { ja: '開発' },
					items: ['architecture', 'adding-a-device', 'contributing', 'security'],
				},
				{
					label: 'Releases',
					translations: { ja: 'リリース' },
					link: `${repo}/releases`,
					attrs: { target: '_blank' },
				},
			],
		}),
	],
});
