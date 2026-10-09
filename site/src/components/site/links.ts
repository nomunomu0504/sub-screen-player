// Links that follow wherever the site is deployed (a custom domain, or <owner>.github.io/<repo>
// with a base path), so pages never hard-code it.
import type { Lang } from '../landing/copy';

export const repo = 'https://github.com/nomunomu0504/sub-screen-player';

const base = import.meta.env.BASE_URL.replace(/\/$/, '');

/** A page of the site in a language, e.g. `page('cli/', 'ja')` is `/ja/cli/`. */
export const page = (path: string, lang: Lang) => `${base}/${lang === 'ja' ? 'ja/' : ''}${path}`;

/** A file at the root of the site, e.g. `/favicon.svg`. */
export const file = (path: string) => `${base}/${path}`;

/** The site's origin, e.g. `https://subscreen.dev`. */
export const origin = (site: URL | undefined) => site?.origin ?? 'https://subscreen.dev';

/** The absolute URL of a path (as given by `page` or `file`). */
export const absolute = (path: string, site: URL | undefined) => new URL(path, origin(site)).href;
