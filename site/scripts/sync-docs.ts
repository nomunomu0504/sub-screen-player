// Builds src/content/docs/ (Starlight's content folder) from two sources, so every document
// has exactly one copy in the repository:
//   - site/content/: pages written for the website (getting started, download; the home page is
//     src/pages/index.astro and ja/index.astro)
//   - the repository's Markdown docs (docs/, CONTRIBUTING.md, SECURITY.md), English and
//     Japanese (*.ja.md), which stay readable on GitHub as they are.
// Repository docs get a frontmatter title from their first heading, lose the language switch
// line (the site has its own), and have their links rewritten: links between published docs
// point to site pages, pictures in docs/images/ to their copies (below), everything else to the
// file on GitHub.
// The pictures of the screens in docs/images/ (also shown in the READMEs) are copied to
// src/assets/screens/ so pages can import them.
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, posix } from 'node:path';

const repoUrl = 'https://github.com/nomunomu0504/sub-screen-player';
const siteDir = join(import.meta.dir, '..');
const root = join(siteDir, '..');
const out = join(siteDir, 'src/content/docs');
const screens = join(siteDir, 'src/assets/screens');

/** Repository docs published on the site: source (English) -> page slug. */
const published: Record<string, string> = {
	'docs/cli.md': 'cli',
	'docs/api.md': 'api',
	'docs/architecture.md': 'architecture',
	'docs/adding-a-device.md': 'adding-a-device',
	'docs/devices/d92.md': 'devices/d92',
	'CONTRIBUTING.md': 'contributing',
	'SECURITY.md': 'security',
};

/** Shorter sidebar labels than the documents' own titles. */
const sidebarLabels: Record<string, string> = {
	contributing: 'Contributing',
	'ja/contributing': 'コントリビュート',
	security: 'Security',
	'ja/security': 'セキュリティ',
	'devices/d92': 'D92',
	'ja/devices/d92': 'D92',
};

const japanese = (path: string) => path.replace(/\.md$/, '.ja.md');

/** Every published file, English and Japanese, -> slug. */
const slugs = new Map<string, string>();
for (const [source, slug] of Object.entries(published)) {
	slugs.set(source, slug);
	slugs.set(japanese(source), `ja/${slug}`);
}

function rewriteLink(target: string, source: string, slug: string): string {
	if (/^([a-z]+:|#)/i.test(target)) return target;
	const [path, anchor] = target.split('#', 2);
	const resolved = posix.normalize(posix.join(posix.dirname(source), path));
	const hash = anchor === undefined ? '' : `#${anchor}`;
	if (resolved.startsWith('docs/images/')) {
		const picture = `src/assets/screens/${resolved.slice('docs/images/'.length)}`;
		return posix.relative(posix.dirname(`src/content/docs/${slug}`), picture);
	}
	const page = slugs.get(resolved);
	if (page) return `${posix.relative(slug, page) || '.'}/${hash}`;
	const kind = existsSync(join(root, resolved)) && statSync(join(root, resolved)).isDirectory() ? 'tree' : 'blob';
	return `${repoUrl}/${kind}/main/${resolved}${hash}`;
}

function convert(source: string, slug: string): string {
	let text = readFileSync(join(root, source), 'utf8');
	const heading = text.match(/^# (.+)$/m);
	if (!heading) throw new Error(`${source} has no top-level heading`);
	text = text.replace(heading[0], '');
	text = text.replace(/^\[(English|日本語)\]\([^)]*\)[ \t]*$/m, '');
	text = text.replace(/\]\(([^)\s]+)\)/g, (_, target: string) => `](${rewriteLink(target, source, slug)})`);
	const frontmatter = [
		'---',
		`title: ${JSON.stringify(heading[1].trim())}`,
		`editUrl: ${repoUrl}/edit/main/${source}`,
		...(sidebarLabels[slug] ? ['sidebar:', `  label: ${JSON.stringify(sidebarLabels[slug])}`] : []),
		'---',
		'',
	].join('\n');
	return frontmatter + text.replace(/^\s+/, '');
}

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
cpSync(join(siteDir, 'content'), out, { recursive: true });

for (const [source, slug] of slugs) {
	if (!existsSync(join(root, source))) throw new Error(`missing ${source}`);
	const file = join(out, `${slug}.md`);
	mkdirSync(dirname(file), { recursive: true });
	writeFileSync(file, convert(source, slug));
}
console.log(`synced ${slugs.size} documents into src/content/docs`);

rmSync(screens, { recursive: true, force: true });
cpSync(join(root, 'docs/images'), screens, { recursive: true });
console.log('copied docs/images into src/assets/screens');
