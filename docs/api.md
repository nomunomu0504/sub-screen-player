# HTTP / WebSocket API

[日本語](api.ja.md)

`ssp serve` exposes this API. The `ssp` CLI uses it too, so anything the CLI does, your
program can do.

- Base URL: `http://127.0.0.1:7920/api/v1` (change with `listen` in the config)
- Bodies are JSON unless noted. Successful actions return `204 No Content`.
- Errors return a JSON body `{"error": "..."}`, malformed requests and unknown paths included.
  Misspelled fields in a JSON body are refused, not ignored.

## Authentication

| Config | Requirement |
|---|---|
| No `token` (default; loopback only) | No credentials. Requests whose `Host` or `Origin` header is not loopback get `403`, so web pages cannot use the API through your browser. |
| `token = "..."` | Send `Authorization: Bearer <token>`, or `?token=<token>` where headers are not possible (browser WebSockets). Otherwise `401`. |

### Pages shown with `ssp web`

A page shown with `ssp web` (or `POST /displays/{id}/web`) can read the daemon's figures, to
draw its own dashboard. Before the page's scripts run, the daemon gives it

```js
window.ssp = { api: "http://127.0.0.1:7920/api/v1", token: "..." }
```

With `Authorization: Bearer <ssp.token>`, the page may `GET` `/health`, `/displays`,
`/displays/{id}`, `/metrics`, `/metrics/{id}` and `/system`, from whatever origin it has
(`null` for a file). These answers carry `Access-Control-Allow-Origin`, and preflight requests
for these paths are answered. Anything else with this token gets `403`: a page cannot change
what is shown, the backlight or the metrics.

```js
const response = await fetch(`${ssp.api}/metrics/claude-code`, {
  headers: { Authorization: `Bearer ${ssp.token}` },
});
```

Each page gets a new token, valid while it is shown (a page reloaded with `reload` keeps it). It
is never written to disk or the log. A page could pass it on to the sites it loads, which could
then read the same figures while it is shown, so show pages you trust.
[contrib/web/system.html](../contrib/web/system.html) is an example.

## Display ids

Ids look like `d92-470B03781D1F` (driver id + USB serial number) and are listed by
`GET /displays`. `default` means the first connected display in id order.

## Endpoints

| Method | Path | Body | Does |
|---|---|---|---|
| `GET` | `/health` | | Daemon status and version |
| `GET` | `/displays` | | All displays seen since the daemon started |
| `GET` | `/displays/{id}` | | One display |
| `POST` | `/displays/{id}/image` | image file | Shows an image (see below) |
| `POST` | `/displays/{id}/clock` | optional JSON | Shows the built-in clock |
| `POST` | `/displays/{id}/dashboard` | optional JSON | Shows the built-in dashboard |
| `POST` | `/displays/{id}/brightness` | `{"percent": 0-100}` | Sets the backlight |
| `POST` | `/displays/{id}/power` | `{"on": true \| false}` | Switches the screen on or off |
| `POST` | `/displays/{id}/clear` | | Stops the current content and blanks the screen |
| `POST` | `/displays/{id}/stop` | | Stops the current content; the screen keeps its picture |
| `GET` | `/displays/{id}/stream` | | WebSocket for live frames (see below) |
| `POST` | `/displays/{id}/notify` | JSON | Shows a message over the screen for a while (see below) |
| `DELETE` | `/displays/{id}/notify` | | Ends the notification |
| `POST` | `/displays/{id}/web` | `{"url": "...", "reload": 600}` | Shows a web page (see below) |
| `GET` | `/web/chrome` | | Whether headless Chrome is ready |
| `POST` | `/web/chrome` | | Downloads headless Chrome |
| `PUT` | `/metrics/{id}` | JSON | Sets a figure for `metric:<id>` dashboard panels (see below) |
| `GET` | `/metrics` | | All metrics |
| `GET` | `/metrics/{id}` | | One metric |
| `DELETE` | `/metrics/{id}` | | Removes a metric |
| `GET` | `/system` | | CPU, memory, network and disk figures (see below) |
| `GET` | `/schedule` | | The last and the next schedule entries (see below) |
| `POST` | `/schedule/pause` | | Stops the changes at set times |
| `POST` | `/schedule/resume` | | Applies the schedule now and starts the changes again |

### `GET /health`

```json
{"status": "ok", "version": "0.1.1", "drivers": ["d92"]}
```

`drivers` lists the driver ids the daemon uses; it only touches displays of these drivers
(see `--driver` in the [command line guide](cli.md)). Daemons before 0.1.1 omit it.

### `GET /displays`

```json
[
  {
    "id": "d92-470B03781D1F",
    "driver": "d92",
    "model": "upHere D92",
    "serial": "470B03781D1F",
    "firmware": "V25.upHere_gamingD92.02.014",
    "connected": true,
    "width": 1920,
    "height": 462,
    "content": "clock",
    "capabilities": {
      "live_frames": true, "saved_frames": true, "brightness": true,
      "power": true, "clear": true, "max_fps": 60
    },
    "stats": {
      "submitted": 925, "shown": 908, "partial": 850, "dropped": 17, "duplicates": 0,
      "last_encode_ms": 5.39, "last_send_ms": 21.85, "last_bytes": 50940
    }
  }
]
```

- `width` x `height` is the size to draw at (landscape). Other sizes are fitted.
- `content` is `nothing`, `image`, `animation`, `video`, `web`, `clock`, `dashboard`, `rotation` or `stream`. It is remembered while the display is
  unplugged (`connected: false`, `stats: null`).
- `stats`: `dropped` counts frames replaced by newer ones before they could be sent;
  `duplicates` counts frames skipped because nothing changed; `partial` counts frames sent as
  their changed parts only (`[display] partial_updates`).
- `notification` appears while a notification is shown (see below):
  `{"text": "CI failed", "style": "banner", "color": "#dc2626", "seconds_left": 8, "sticky": false}`,
  with `detail` if it has one and without `seconds_left` if it is sticky.

### `POST /displays/{id}/image`

The body is a PNG, JPEG, GIF or WebP file (up to 64 MiB). An animated GIF, APNG or WebP plays in
a loop, each frame for its own delay (the display's `content` is then `animation`), up to
10,000 frames. Animations that take more than 256 MiB decoded (a full-screen 60 fps animation
longer than about a second) are decoded again while they play instead of being kept in memory.
Query parameters:

| Parameter | Values | Default |
|---|---|---|
| `fit` | `contain` (letterbox), `cover` (crop), `stretch` | `contain` |
| `persist` | `true` also stores the image on the device (survives power loss, writes flash); the first frame of an animation | `false` |

```sh
curl --data-binary @photo.png "http://127.0.0.1:7920/api/v1/displays/default/image?fit=cover"
```

The body can also be a video (MP4, QuickTime, WebM, Matroska, AVI or MPEG-TS, up to 4 GiB),
played in a loop by the [ffmpeg](https://ffmpeg.org/) installed on the computer (`content` is
then `video`). The daemon keeps the file in a temporary folder while it plays and checks that
ffmpeg can decode it before answering: a file it cannot play gets `400` with ffmpeg's message,
and a missing ffmpeg `501` with how to install it. `persist` is refused for videos. ffmpeg is
looked up in `[video] ffmpeg` of the config, then on `PATH`, then where it is usually installed:
`/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin` and `/snap/bin`, or on Windows the WinGet,
Scoop and Chocolatey folders.

### `POST /displays/{id}/clock`

All fields are optional; missing ones come from the `[clock]` section of the config.

```json
{
  "seconds": false,
  "time_format": "%H:%M",
  "date_format": "%A, %B %-d",
  "color": "#FFD080",
  "background": "#101018"
}
```

Formats use strftime syntax ([reference](https://docs.rs/jiff/latest/jiff/fmt/strtime/)).
An empty `date_format` hides the date. `weekdays` (7 names, from Sunday) replaces what `%a` and
`%A` print, e.g. `["日", "月", "火", "水", "木", "金", "土"]`.

### `POST /displays/{id}/dashboard`

All fields are optional; missing ones come from the `[dashboard]` section of the config. The
clock panel uses the formats of `[clock]`.

```json
{
  "widgets": ["clock", "cpu", "memory", "network", "disk"],
  "color": "#F0F2F8",
  "accent": "#6EE7B7",
  "background": "#000000"
}
```

`widgets` lists the panels from left to right (1 to 6 of `clock`, `cpu`, `memory`, `network`,
`disk`, `claude-code` and `metric:<id>`; see the [CLI guide](cli.md#show-how-much-claude-code-you-have-used)
for `claude-code`). `accent` is the color of the graphs.

### `GET /schedule`, `POST /schedule/pause`, `POST /schedule/resume`

The schedule of the config (`[[schedule]]`, see the [command line guide](cli.md)):

```json
{
  "entries": 4,
  "paused": false,
  "last": { "at": "2026-10-09T19:00:00+09:00", "entries": [2], "does": ["show clock, brightness 40"] },
  "next": { "at": "2026-10-10T01:00:00+09:00", "entries": [3], "does": ["power off"] }
}
```

`entries` in `last` and `next` are positions in the config, from 1; `at` is local time. Without
a schedule, `entries` is 0 and `last` and `next` are missing. `pause` stops the changes at set
times; `resume` applies what the schedule says now to every display and starts them again. Both
answer `409` without a schedule.

### `POST /displays/{id}/notify`, `DELETE /displays/{id}/notify`

Shows a message over whatever the display shows, then goes back to it. The content keeps
running underneath; a new notification replaces the current one.

```json
{ "text": "CI failed", "detail": "main · 2 of 41 jobs", "seconds": 30, "color": "red" }
```

| Field | Meaning |
|---|---|
| `text` | The message, 1 to 80 characters. Required. |
| `detail` | A smaller line under it, up to 120 characters. |
| `seconds` | How long it stays, 1 to 86400 (default 10). |
| `sticky` | `true`: it stays until dismissed or replaced, instead of `seconds`. |
| `style` | `banner` (the bottom third, default) or `full` (the whole panel). |
| `color` | Background: `red`, `orange`, `yellow`, `green`, `blue` (default), `gray` or `#rrggbb`. |
| `wake` | `true`: a screen that is off is switched on while it is shown, and off again after. |

A notification belongs to the display, not to its content: it stays when the content changes,
and while the display is unplugged (it shows again when the display is back, if it has not
ended). Over nothing (`content` is `nothing`, or after `stop`) it is drawn on the last picture,
black if there is none. `POST /displays/{id}/clear` ends it with the content, and
`DELETE /displays/{id}/notify` ends it at once (`204` whether there was one or not). A page
token can read it in `GET /displays` but not post one.

### Metrics

Metrics are figures your own scripts send, shown by the dashboard's `metric:<id>` panels: a CI
status, a queue length, a price, the room temperature. Send a new value whenever it changes; the
panel redraws within a second.

```sh
curl -X PUT http://127.0.0.1:7920/api/v1/metrics/ci \
  -H 'Content-Type: application/json' \
  -d '{"label": "CI", "value": 2, "unit": "failed", "detail": "main · 39 of 41 jobs passed"}'
```

`PUT /metrics/{id}` takes these fields, all optional. Fields left out keep their previous value;
creating a metric needs one of `value`, `text` and `series`.

| Field | Type | Meaning |
|---|---|---|
| `value` | number | The figure shown. Each value is also added to the panel's graph (the last 60 are kept). |
| `text` | string (≤ 40) | A short text shown instead of a number, e.g. `"passing"`. Clears the graph. |
| `series` | numbers (≤ 1000) | Replaces the graph's values, oldest first; the last one becomes `value` unless `value` is given too. |
| `label` | string (≤ 40) | Name above the value. Defaults to the id. |
| `unit` | string (≤ 16) | Shown small after the value, e.g. `"%"`. |
| `detail` | string (≤ 120) | The line under the value. |
| `max` | number | Top of the graph. Defaults to the largest value in the graph. |
| `ttl` | seconds | How long the value counts as current (default 300, at most a week). After that the panel is dimmed and says how old the value is. |

Numbers of 100,000 or more are shortened on the panel (`123k`, `4.56M`, `7.8B`). Ids are 1 to 32
of `a-z`, `0-9` and `-`. The daemon keeps at most 64 metrics, in memory only: they
are gone after a restart, so send them again (most senders run on a timer anyway). A
`metric:<id>` panel whose metric has not arrived yet says "waiting for data".

`GET /metrics` returns all metrics sorted by id, and `GET /metrics/{id}` one:

```json
{
  "id": "ci",
  "label": "CI",
  "value": 2.0,
  "unit": "failed",
  "detail": "main · 39 of 41 jobs passed",
  "ttl": 300,
  "updated": "2026-10-08T10:24:49.809733Z",
  "age": 12,
  "stale": false,
  "history": [0.0, 3.0, 2.0]
}
```

`text` replaces `value` when set; `max` appears when set. `age` is in seconds.
`DELETE /metrics/{id}` returns `404` for an unknown id.

Once a dashboard shows the `claude-code` panel or the metric is asked for with
`GET /metrics/claude-code`, the daemon keeps the metric `claude-code` up to date itself (tokens in the current 5-hour block, the time it ends and today's total in `detail`,
and tokens per minute over the last hour in `history`).

### `GET /system`

The figures the dashboard draws:

```json
{
  "cpu_percent": 12.5,
  "cpu_count": 10,
  "load": 2.31,
  "memory_used": 25769803776,
  "memory_total": 68719476736,
  "rx_per_sec": 1250000.0,
  "tx_per_sec": 84000.0,
  "disk_used": 512000000000,
  "disk_total": 994662584320
}
```

CPU use (0-100, over all cores) and the network rates (bytes per second over all interfaces but
loopback) are averaged since the previous request; ask about once a second. Memory and disk are in
bytes; the disk is the one holding the system (`/`, or `C:\` on Windows). `load` (one-minute load
average) is `null` on Windows, and `disk_used` and `disk_total` are `null` if the disk is not
found.

### `POST /displays/{id}/web`

Shows a web page, drawn by headless Chrome at the panel's size (`content` is then `web`).

```json
{ "url": "https://example.com/status", "reload": 600 }
```

`url` is an `http`, `https` or `file` URL; `reload` (optional) reloads the page every so many
seconds. The browser starts after the answer: a page that cannot be loaded is reported on the
display and in the daemon's log. Without a browser the answer is `409`, unless
`[web] auto_download = true` lets the daemon download one first.

### `GET /web/chrome`, `POST /web/chrome`

`GET` tells whether a browser is ready; `POST` downloads the current headless Chrome from Chrome
for Testing (about 100 MB; the request lasts until it is done, and answers `502` if the download
fails), and both return:

```json
{
  "installed": true,
  "path": "/Users/me/Library/Application Support/sub-screen-player/chrome/155.0.8059.39/chrome-headless-shell-mac-arm64/chrome-headless-shell",
  "version": "155.0.8059.39",
  "configured": false,
  "dir": "/Users/me/Library/Application Support/sub-screen-player/chrome"
}
```

`configured` is `true` when the browser comes from `[web] chrome` (then there is no `version`).

## WebSocket stream

`GET /displays/{id}/stream` upgrades to a WebSocket. Every **binary** message is one frame:

| `format` query parameter | Message contents |
|---|---|
| `image` (default) | An image file (PNG, JPEG, GIF, WebP) of any size, fitted with `fit` |
| `rgb` | Raw 8-bit RGB, exactly `width * height * 3` bytes |
| `rgba` | Raw 8-bit RGBA, exactly `width * height * 4` bytes; alpha is blended onto black |

- Send at any rate. The display shows the newest frame it can (up to `max_fps`); older ones
  are dropped. JPEG at about the panel size is the cheapest format for the daemon to handle.
- While a stream is connected, the display's content is `stream`. Setting other content (an
  image, the clock, the dashboard, `stop`) ends the stream: the next frame is answered with close code
  `4000`.
- A bad frame does not end the stream. The daemon replies with a text message
  `{"error": "..."}` and keeps going.
- The stream survives the display being replugged; frames sent while it is unplugged are
  answered with an error.
- The stream closes when the daemon shuts down.

Browser example (a page served from `localhost`):

```js
const ws = new WebSocket("ws://127.0.0.1:7920/api/v1/displays/default/stream");
const canvas = Object.assign(document.createElement("canvas"), { width: 1920, height: 462 });
const ctx = canvas.getContext("2d");

ws.onopen = () => setInterval(() => {
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, 1920, 462);
  ctx.fillStyle = "#fff";
  ctx.font = "200px sans-serif";
  ctx.fillText(new Date().toLocaleTimeString(), 80, 300);
  canvas.toBlob((blob) => ws.send(blob), "image/jpeg", 0.85);
}, 1000 / 30);
ws.onmessage = (event) => console.warn("ssp:", event.data);
```

Browsers can use the WebSocket from `localhost` pages. The HTTP endpoints do not send CORS
headers yet, so call them from scripts or the CLI rather than from web pages.
