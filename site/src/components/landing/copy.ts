// Text of the landing page in English and Japanese. Japanese headings are split into phrases
// that are kept on one line each (`<span class="ph">`), so they wrap between phrases.

export type Lang = 'en' | 'ja';

/** A heading: one string, or phrases that each stay on one line. */
export type Heading = string | string[];

interface Screen {
	image: string;
	alt: string;
	title: string;
	text: string;
	command: string;
}

export interface Copy {
	htmlLang: string;
	title: string;
	description: string;
	nav: { home: string; sections: string; screens: string; how: string; docs: string; language: string };
	hero: {
		eyebrow: string;
		title: Heading;
		lead: [string, string];
		start: string;
		download: string;
		os: string;
		copy: string;
		copied: string;
		note: [string, string, string];
		alts: [string, string, string];
	};
	screens: { eyebrow: string; title: Heading; text: string; layout: Screen; cards: Screen[]; schedulesTitle: string; schedulesText: string };
	claude: {
		eyebrow: string;
		title: Heading;
		text: string;
		items: [string, string][];
		note: string;
		notifyAlt: string;
		notifyCaption: string;
		usageAlt: string;
		usageCaption: string;
	};
	how: {
		eyebrow: string;
		title: Heading;
		text: string;
		partialTitle: string;
		partialText: string;
		partialAria: string;
		whole: string;
		parts: string;
		about: string;
		partialNote: string;
		cards: [string, string][];
	};
	api: {
		eyebrow: string;
		title: Heading;
		text: string;
		sendTitle: string;
		sendAlt: string;
		readTitle: string;
		readComment: string;
		readNote: string;
		reference: string;
	};
	displays: {
		eyebrow: string;
		title: Heading;
		text: string;
		detail: string;
		supported: string;
		yoursTitle: string;
		yoursText: string;
		adding: string;
		trademark: string;
	};
	gallery: { eyebrow: string; soon: string; title: Heading; text: string; yours: string };
	closing: { title: Heading };
	footer: { about: string; project: string; license: string; docs: string; cliGuide: string; privacy: string };
}

export const copy: Record<Lang, Copy> = {
	en: {
		htmlLang: 'en',
		title: 'sub-screen-player — light up the bar display under your monitor',
		description:
			'Drive long, narrow USB sub-displays like the upHere D92 from macOS, Linux and Windows: a clock, a system dashboard, Claude Code status, videos and web pages at up to 60 fps.',
		nav: { home: 'sub-screen-player home', sections: 'Sections', screens: 'Screens', how: 'How it works', docs: 'Docs', language: 'Language' },
		hero: {
			eyebrow: 'Open source · macOS · Linux · Windows',
			title: 'Light up the bar display under your monitor.',
			lead: [
				'sub-screen-player (',
				') drives long, narrow USB sub-displays like the 1920×462 upHere D92. A small daemon on your PC sends the clock, a system dashboard, Claude Code status, video or your own web page — at up to 60 fps, from a single Rust binary.',
			],
			start: 'Get started',
			download: 'Download',
			os: 'Operating system',
			copy: 'Copy',
			copied: 'Copied',
			note: ['Downloads the latest release, checks its SHA-256 checksum and installs ', '. Then run ', ' — the clock appears.'],
			alts: [
				'The display showing a large clock with the date underneath',
				'The display showing the system dashboard: time, CPU, memory, network and disk panels with graphs',
				'The display showing a clock with a blue notification over it: Claude Code is waiting for you',
			],
		},
		screens: {
			eyebrow: '01 — Screens',
			title: 'What you can put on it',
			text: 'Every picture below is real output at 1920×462. One command switches the screen; the daemon keeps it running.',
			layout: {
				image: 'layout',
				alt: 'A layout with the clock on the left, a looping video in the middle and a CI metric panel on the right',
				title: 'Layout',
				text: 'Clock, video and panels side by side. Each zone runs on its own — no browser involved.',
				command: 'ssp layout clock video:loop.mp4 metric:ci',
			},
			cards: [
				{ image: 'clock', alt: 'Clock screen: the time and the date', title: 'Clock', text: 'Shown by default. Pick the format and the colors.', command: 'ssp clock' },
				{ image: 'clockJa', alt: 'Clock screen with a Japanese date', title: 'Japanese dates', text: 'Date formats are set in the config file, Japanese included.', command: 'ssp config init' },
				{ image: 'dashboard', alt: 'Dashboard: time next to CPU, memory, network and disk panels, each with a graph', title: 'System dashboard', text: 'CPU, memory, network and disk, each with a graph of the last minute.', command: 'ssp dashboard' },
				{ image: 'claudeCode', alt: 'Dashboard with a Claude Code panel showing tokens used, next to CPU and memory', title: 'Claude Code usage', text: 'Tokens in the current 5-hour block and today, read from local logs.', command: 'ssp dashboard --widgets clock,claude-code,cpu,memory' },
				{ image: 'metrics', alt: 'Dashboard with three custom panels: CI 2 failed, Deploy live, Queue 26 jobs', title: 'Your own figures', text: 'Send a value from any script — CI status, a queue length — and it becomes a panel.', command: 'ssp metric set ci --value 2 --label CI --unit failed' },
				{ image: 'web', alt: 'A web page on the display: the time and a progress bar of the day', title: 'Web pages', text: "Any HTML page or URL, drawn by headless Chrome. Pages can read the daemon's figures.", command: 'ssp web page.html' },
				{ image: 'video', alt: 'One frame of a colourful fractal video filling the display', title: 'Video, images, animations', text: 'Video through ffmpeg; pictures and animated GIFs fitted to the panel.', command: 'ssp show clip.mp4 --fit cover' },
				{ image: 'notify', alt: 'A blue notification bar over the clock: Claude Code is waiting for you', title: 'Notifications', text: 'A message over whatever is showing, for a few seconds. Then back to what was there.', command: 'ssp notify "Build finished" --color green' },
			],
			schedulesTitle: 'Schedules',
			schedulesText: 'Change the screen, the brightness and the power at set times of day, or show several screens in turn.',
		},
		claude: {
			eyebrow: '02 — Claude Code',
			title: 'Know when Claude Code needs you, without switching windows.',
			text: 'Run one line, once. From then on the bar tells you when Claude Code is waiting for a permission or a reply, and when it has finished.',
			items: [
				['waiting', 'It needs your permission or your answer.'],
				['done', 'The work is finished.'],
				['usage', "A panel with the current 5-hour block and today's tokens."],
			],
			note: "Everything stays on your machine: ssp only reads Claude Code's local logs and sends nothing anywhere. Claude Code is a product of Anthropic; sub-screen-player is an independent project.",
			notifyAlt: 'Notification over the clock: Claude Code is waiting for you — Claude needs your permission to use Bash',
			notifyCaption: 'Notification — Claude Code is waiting',
			usageAlt: 'Dashboard with the Claude Code usage panel',
			usageCaption: 'Usage panel — ssp dashboard --widgets clock,claude-code,cpu,memory',
		},
		how: {
			eyebrow: '03 — How it works',
			title: 'A small local daemon, and not much else.',
			text: 'One binary runs in the background, keeps the display alive and sends it frames over USB. Everything else — the CLI, your scripts, a web page — talks to that daemon.',
			partialTitle: 'Only the changed parts are sent',
			partialText: 'For a clock, most of the picture stays the same from one frame to the next. ssp sends the part that moved.',
			partialAria: 'A clock frame sent whole is about 50 KB; sending only the changed parts takes about 3.5 KB',
			whole: 'whole frame',
			parts: 'changed parts only',
			about: '≈',
			partialNote: 'One clock frame on the D92.',
			cards: [
				['Up to 60 fps', 'A single Rust binary. If frames arrive faster than the display takes them, only the newest is sent.'],
				['HTTP + WebSocket API', 'Draw on the screen from any language: send one image, a stream of frames, or just a number.'],
				['Starts at login', 'Unplug the display and plug it back in — it carries on with what it was showing.'],
				['Localhost only by default', 'The API listens on your own machine. Nothing is sent to the outside.'],
			],
		},
		api: {
			eyebrow: '04 — For developers',
			title: 'If it can make an HTTP request, it can draw.',
			text: 'Push a number from a CI job and it shows up as a panel. Or build the whole screen as a web page and read the same numbers from inside it.',
			sendTitle: 'Send a figure',
			sendAlt: 'The CI figure shown as a dashboard panel: 2 failed',
			readTitle: 'Read it from a web page',
			readComment: '// in a page shown with `ssp web`',
			readNote: 'The daemon gives each page a token that can only read. A page cannot change what is shown.',
			reference: 'HTTP / WebSocket API reference →',
		},
		displays: {
			eyebrow: '05 — Displays',
			title: 'Supported displays',
			text: 'One display today. The code is laid out so that adding the next one means writing only its device protocol.',
			detail: 'MiraBox family · 1920×462 · about 4.2:1',
			supported: 'Supported',
			yoursTitle: 'Your display?',
			yoursText: 'Contributions for other displays are welcome.',
			adding: 'Adding a device →',
			trademark: 'Product names belong to their owners. sub-screen-player is not affiliated with or endorsed by the makers of these displays.',
		},
		gallery: {
			eyebrow: '06 — Screen gallery',
			soon: 'Coming soon',
			title: 'Screens made by everyone, one line away.',
			text: 'A place for screens people have built as web pages. Pick one and point ssp at its URL:',
			yours: 'your screen here',
		},
		closing: { title: 'Plug it in. Run one line.' },
		footer: {
			about: 'Drives long, narrow USB sub-displays from macOS, Linux and Windows. Licensed under MIT OR Apache-2.0.',
			project: 'Project',
			license: 'License',
			docs: 'Docs',
			cliGuide: 'Command line guide',
			privacy: 'This site uses no analytics and no cookies.',
		},
	},
	ja: {
		htmlLang: 'ja',
		title: 'sub-screen-player — 机の上の細長い画面に、いま見たいものを',
		description:
			'upHere D92 のような横長の USB サブディスプレイを、macOS・Linux・Windows から動かすツール。時計、システムのダッシュボード、Claude Code の状態、動画、Web ページを最大 60fps で表示します。',
		nav: { home: 'sub-screen-player トップ', sections: 'セクション', screens: '表示できるもの', how: '仕組み', docs: 'ドキュメント', language: '言語' },
		hero: {
			eyebrow: 'オープンソース · macOS · Linux · Windows',
			title: ['机の上の細長い画面に、', 'いま見たいものを。'],
			lead: [
				'sub-screen-player（',
				'）は、モニターの下に置く横長の USB サブディスプレイ（例: 1920×462 の upHere D92）を動かすツールです。PC 上の小さなデーモンが、時計、システムのダッシュボード、Claude Code の状態、動画、自作の Web ページを最大 60fps で送ります。Rust 製の単一バイナリです。',
			],
			start: 'はじめる',
			download: 'ダウンロード',
			os: 'OS',
			copy: 'コピー',
			copied: 'コピー済み',
			note: ['最新のリリースをダウンロードし、SHA-256 チェックサムを確かめて、', ' をインストールします。続けて ', ' を実行すると、時計が表示されます。'],
			alts: [
				'大きな時計と、日本語の日付を表示したディスプレイ',
				'時刻と CPU・メモリ・ネットワーク・ディスクのグラフを並べたダッシュボードを表示したディスプレイ',
				'時計の上に青い通知「Claude Code is waiting for you」を重ねたディスプレイ',
			],
		},
		screens: {
			eyebrow: '01 — 表示できるもの',
			title: 'この画面に出せるもの',
			text: '下の画像は、すべて 1920×462 の実際の表示です。コマンド 1 つで画面が切り替わり、あとはデーモンが表示を続けます。',
			layout: {
				image: 'layout',
				alt: '左に時計、中央に動画、右に CI の数値パネルを並べたレイアウト',
				title: 'レイアウト',
				text: '時計・動画・パネルを横に並べます。それぞれが独立して動き、ブラウザは使いません。',
				command: 'ssp layout clock video:loop.mp4 metric:ci',
			},
			cards: [
				{ image: 'clock', alt: '時計の画面。時刻と日付', title: '時計', text: '最初に表示される画面です。書式と色を変えられます。', command: 'ssp clock' },
				{ image: 'clockJa', alt: '日本語の日付を表示した時計の画面', title: '日本語の日付', text: '日付の書式は設定ファイルで変えられます。日本語にも対応しています。', command: 'ssp config init' },
				{ image: 'dashboard', alt: '時刻の横に CPU・メモリ・ネットワーク・ディスクのパネルとグラフを並べたダッシュボード', title: 'システムのダッシュボード', text: 'CPU・メモリ・ネットワーク・ディスクを、直近 1 分のグラフと一緒に表示します。', command: 'ssp dashboard' },
				{ image: 'claudeCode', alt: 'Claude Code のトークン数のパネルを CPU・メモリと並べたダッシュボード', title: 'Claude Code の使用量', text: '今の 5 時間枠と今日のトークン数を、手元のログから読み取って表示します。', command: 'ssp dashboard --widgets clock,claude-code,cpu,memory' },
				{ image: 'metrics', alt: 'CI 2 failed、Deploy live、Queue 26 jobs の 3 つのパネルを並べたダッシュボード', title: 'スクリプトから送った数値', text: 'CI の状態やキューの長さなどを送ると、そのままパネルになります。', command: 'ssp metric set ci --value 2 --label CI --unit failed' },
				{ image: 'web', alt: '時刻と 1 日の進み具合のバーを表示した Web ページの画面', title: 'Web ページ', text: 'HTML のページや URL を、ヘッドレス Chrome で描画します。ページからデーモンの数値も読めます。', command: 'ssp web page.html' },
				{ image: 'video', alt: '画面いっぱいに表示した、色鮮やかなフラクタルの動画の 1 コマ', title: '動画・画像・アニメーション', text: '動画は ffmpeg で再生します。画像や GIF アニメーションは、画面に合わせて表示します。', command: 'ssp show clip.mp4 --fit cover' },
				{ image: 'notify', alt: '時計の上に重ねた青い通知バー「Claude Code is waiting for you」', title: '通知', text: '表示中の内容の上にメッセージを数秒重ね、そのあと元の表示に戻ります。', command: 'ssp notify "Build finished" --color green' },
			],
			schedulesTitle: 'スケジュール',
			schedulesText: '時刻に合わせて、表示・明るさ・点灯を切り替えます。複数の画面を順番に表示することもできます。',
		},
		claude: {
			eyebrow: '02 — Claude Code',
			title: ['Claude Code が待っていることに、', 'ウィンドウを切り替えずに気づく。'],
			text: '次の 1 行を、一度だけ実行してください。Claude Code が許可や返答を待っているときと、作業を終えたときに、バーに通知が出ます。',
			items: [
				['入力待ち', '許可や返答を待っています。'],
				['完了', '作業が終わりました。'],
				['使用量', '今の 5 時間枠と今日のトークン数を、パネルで表示します。'],
			],
			note: 'すべて手元で完結します。ssp は Claude Code のローカルのログを読むだけで、外部には何も送りません。Claude Code は Anthropic の製品です。sub-screen-player は独立したプロジェクトです。',
			notifyAlt: '時計の上に重ねた通知「Claude Code is waiting for you — Claude needs your permission to use Bash」',
			notifyCaption: '通知 — Claude Code が入力を待っているとき',
			usageAlt: 'Claude Code の使用量のパネルを並べたダッシュボード',
			usageCaption: '使用量のパネル — ssp dashboard --widgets clock,claude-code,cpu,memory',
		},
		how: {
			eyebrow: '03 — 仕組み',
			title: ['動いているのは、', '手元の小さなデーモンだけ。'],
			text: 'バイナリ 1 つがバックグラウンドで動き、USB でディスプレイにフレームを送り続けます。CLI も、スクリプトも、Web ページも、このデーモンに話しかけます。',
			partialTitle: '変わった部分だけを送る',
			partialText: '時計の画面は、前のフレームからほとんど変わりません。ssp は、変わった部分だけを送ります。',
			partialAria: '時計の 1 フレームは、全体を送ると約 50 KB、変わった部分だけなら約 3.5 KB',
			whole: 'フレーム全体',
			parts: '変わった部分だけ',
			about: '約',
			partialNote: 'D92 で時計を表示したときの 1 フレーム。',
			cards: [
				['最大 60fps', 'Rust 製の単一バイナリです。ディスプレイが受け取れる速さを超えたら、最新のフレームだけを送ります。'],
				['HTTP + WebSocket API', 'どの言語からでも描画できます。画像 1 枚でも、連続したフレームでも、数値 1 つでも。'],
				['ログイン時に自動起動', 'ディスプレイを抜き差ししても、表示していた内容をそのまま続けます。'],
				['初期設定は localhost のみ', 'API は自分の PC の中だけで待ち受けます。外部には何も送りません。'],
			],
		},
		api: {
			eyebrow: '04 — 開発者向け',
			title: ['HTTP リクエストを送れるなら、', '描画できます。'],
			text: 'CI のジョブから数値を送れば、パネルとして表示されます。画面全体を Web ページで作り、その中から同じ数値を読むこともできます。',
			sendTitle: '数値を送る',
			sendAlt: '送った CI の数値が「2 failed」のパネルとして表示されたダッシュボード',
			readTitle: 'Web ページから読む',
			readComment: '// `ssp web` で表示したページの中で',
			readNote: 'デーモンは、表示するページごとに読み取り専用のトークンを渡します。ページから表示内容を変えることはできません。',
			reference: 'HTTP / WebSocket API リファレンス →',
		},
		displays: {
			eyebrow: '05 — ディスプレイ',
			title: '対応ディスプレイ',
			text: '今は 1 機種です。新しいディスプレイには、その機種固有のプロトコルを書くだけで対応できる作りにしています。',
			detail: 'MiraBox 系 · 1920×462 · 約 4.2:1',
			supported: '対応済み',
			yoursTitle: 'ほかのディスプレイをお持ちですか？',
			yoursText: '新しいディスプレイへの対応を歓迎しています。',
			adding: 'ディスプレイを追加する →',
			trademark: '製品名は各社の商標です。sub-screen-player は、これらのディスプレイのメーカーと提携しておらず、公認も受けていません。',
		},
		gallery: {
			eyebrow: '06 — 画面ギャラリー',
			soon: '近日公開',
			title: ['みんなが作った画面を、', '1 行で。'],
			text: 'Web ページとして作られた画面を並べる場所です。気に入った画面の URL を ssp に渡すだけで使えます。',
			yours: 'あなたの画面',
		},
		closing: { title: ['つないで、', '1 行実行するだけ。'] },
		footer: {
			about: '横長の USB サブディスプレイを、macOS・Linux・Windows から動かすツール。ライセンスは MIT OR Apache-2.0 です。',
			project: 'プロジェクト',
			license: 'ライセンス',
			docs: 'ドキュメント',
			cliGuide: 'コマンドラインガイド',
			privacy: 'このサイトは、アクセス解析も Cookie も使いません。',
		},
	},
};
