// https://subscreen.dev/latest.json: the latest release, which `ssp status` checks at most once a
// day to tell when a newer version is out (issue #18). Written when the site is built.
import type { APIRoute } from 'astro';
import { latestRelease } from '../lib/release';

export const GET: APIRoute = async () => {
	const release = await latestRelease();
	if (!release) {
		// A deploy must not publish an empty file; a local build without network may.
		if (process.env.CI) throw new Error('latest.json: cannot read the latest release from GitHub');
		return new Response('{}\n', { headers: { 'Content-Type': 'application/json' } });
	}
	const body = {
		version: release.tag_name.replace(/^v/, ''),
		tag: release.tag_name,
		published: release.published_at,
		url: release.html_url,
	};
	return new Response(`${JSON.stringify(body)}\n`, { headers: { 'Content-Type': 'application/json' } });
};
