// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

const repo = 'https://github.com/nomunomu0504/sub-screen-player';

// Set by the deploy workflow. With a custom domain the site lives at the root; on
// github.io it lives under /<repository>.
const site = process.env.SITE_URL ?? 'https://nomunomu0504.github.io';
const base = process.env.SITE_BASE ?? '/sub-screen-player';

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
