// The latest release of sub-screen-player, read from the GitHub API once per build, for the
// download page and latest.json (the deploy workflow also runs when a release is published).

export const repo = 'nomunomu0504/sub-screen-player';

export type Asset = { name: string; browser_download_url: string; size: number };
export type Release = { tag_name: string; html_url: string; published_at: string; assets: Asset[] };

let latest: Promise<Release | undefined> | undefined;

/** The latest release, or nothing when it cannot be read (an offline build). */
export function latestRelease(): Promise<Release | undefined> {
	latest ??= (async () => {
		try {
			const headers: Record<string, string> = { Accept: 'application/vnd.github+json' };
			if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
			const response = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, { headers });
			return response.ok ? ((await response.json()) as Release) : undefined;
		} catch {
			return undefined;
		}
	})();
	return latest;
}
