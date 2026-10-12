// The screens of the gallery: pages in public/screens/<name>/, served at /screens/<name>/, with
// a picture in src/assets/gallery/<name>.png (scripts/shoot-screens.sh). To add one, add its
// folder, its picture and an entry here.
import type { ImageMetadata } from 'astro';
import type { Lang } from '../landing/copy';
import system from '../../assets/gallery/system.png';
import claudeCode from '../../assets/gallery/claude-code.png';
import metrics from '../../assets/gallery/metrics.png';
import weather from '../../assets/gallery/weather.png';
import day from '../../assets/gallery/day.png';
import calendar from '../../assets/gallery/calendar.png';
import worldClock from '../../assets/gallery/world-clock.png';
import countdown from '../../assets/gallery/countdown.png';

export interface Screen {
	name: string;
	image: ImageMetadata;
	/** What it reads from the daemon (nothing for a screen that only shows the time). */
	reads: string[];
	title: Record<Lang, string>;
	text: Record<Lang, string>;
	/** Another way to call it, if it takes options. */
	variant?: string;
}

export const screens: Screen[] = [
	{
		name: 'system',
		image: system,
		reads: ['GET /system'],
		title: { en: 'System', ja: 'システム' },
		text: {
			en: 'The time, CPU, memory and disk as rings, and the network over the last minute.',
			ja: '時刻と、CPU・メモリ・ディスクのリング、直近 1 分の通信量のグラフ。',
		},
	},
	{
		name: 'claude-code',
		image: claudeCode,
		reads: ['GET /metrics/claude-code'],
		title: { en: 'Claude Code', ja: 'Claude Code' },
		text: {
			en: 'Tokens in the current 5-hour block, when it ends, today’s total and every minute of the last hour. Read from Claude Code’s local logs; nothing leaves your computer.',
			ja: '今の 5 時間枠のトークン数と終わる時刻、今日の合計、直近 1 時間の 1 分ごとの量。Claude Code の手元のログから読み、外部には何も送りません。',
		},
	},
	{
		name: 'metrics',
		image: metrics,
		reads: ['GET /metrics'],
		title: { en: 'Metrics', ja: 'メトリクス' },
		text: {
			en: 'The figures your scripts send (ssp metric set) as tiles with a graph each, up to 8. Pick and order them with ?ids=.',
			ja: 'スクリプトから送った数値（ssp metric set）を、グラフ付きのタイルで最大 8 つ。?ids= で選んで並べ替えられます。',
		},
		variant: 'ssp web "https://subscreen.dev/screens/metrics/?ids=ci,deploy,queue"',
	},
	{
		name: 'weather',
		image: weather,
		reads: [],
		title: { en: 'Weather', ja: '天気' },
		text: {
			en: 'The weather now, the next 24 hours and the next days, from Open-Meteo (free, no key). Give your place with ?lat=, ?lon= and ?name= (Tokyo if not); ?f for Fahrenheit. Only the place goes to Open-Meteo.',
			ja: 'いまの天気と、この先 24 時間・数日の予報。Open-Meteo（無料・キー不要）から読みます。場所は ?lat=・?lon=・?name= で指定し（省略すると東京）、?f で華氏になります。Open-Meteo に送るのは場所だけです。',
		},
		variant: 'ssp web "https://subscreen.dev/screens/weather/?lat=34.69&lon=135.50&name=Osaka"',
	},
	{
		name: 'day',
		image: day,
		reads: [],
		title: { en: 'Day', ja: '1 日' },
		text: {
			en: 'The time, the date and how much of the day has gone. Needs nothing from the daemon.',
			ja: '時刻と日付、1 日のうちどれだけ過ぎたか。デーモンの数値は使いません。',
		},
	},
	{
		name: 'calendar',
		image: calendar,
		reads: [],
		title: { en: 'Calendar', ja: 'カレンダー' },
		text: {
			en: 'The time and the date with this month and the next, today marked, and the week of the year. Weeks start on Sunday; ?monday starts them on Monday.',
			ja: '時刻と日付に、今月と来月のカレンダー（今日に印）と、年の何週目か。週は日曜始まりで、?monday で月曜始まりになります。',
		},
		variant: 'ssp web "https://subscreen.dev/screens/calendar/?monday"',
	},
	{
		name: 'world-clock',
		image: worldClock,
		reads: [],
		title: { en: 'World clock', ja: '世界時計' },
		text: {
			en: 'The time in up to six places, with their date, how far they are from your time and whether it is day or night there. Give the time zones with ?zones= (Tokyo, London, New York and Los Angeles if not) and their names with ?labels=.',
			ja: '最大 6 か所の時刻と日付、手元の時刻との差、昼か夜か。タイムゾーンは ?zones= で指定し（省略すると東京・ロンドン・ニューヨーク・ロサンゼルス）、?labels= で表示名を付けられます。',
		},
		variant: 'ssp web "https://subscreen.dev/screens/world-clock/?zones=Asia/Tokyo,Asia/Singapore,Europe/Berlin&labels=Tokyo,Singapore,Berlin"',
	},
	{
		name: 'countdown',
		image: countdown,
		reads: [],
		title: { en: 'Countdown', ja: 'カウントダウン' },
		text: {
			en: 'Days, hours and minutes until a moment, and how long ago it was once it has passed. Give it with ?to= (2026-12-24T18:00, your time) and name it with ?label=; without them, it counts to the new year.',
			ja: 'ある時刻までの日数・時間・分と、過ぎてからはその経過時間。時刻は ?to=（2026-12-24T18:00 のように手元の時刻で）、名前は ?label= で指定します。省略すると新年までを数えます。',
		},
		variant: 'ssp web "https://subscreen.dev/screens/countdown/?to=2026-12-24T18:00&label=Party"',
	},
];
