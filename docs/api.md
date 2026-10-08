# HTTP / WebSocket API

[日本語](api.ja.md)

`ssp serve` exposes this API. The `ssp` CLI uses it too, so anything the CLI does, your
program can do.

- Base URL: `http://127.0.0.1:7920/api/v1` (change with `listen` in the config)
- Bodies are JSON unless noted. Successful actions return `204 No Content`.
- Errors return a JSON body `{"error": "..."}`.

## Authentication

| Config | Requirement |
|---|---|
| No `token` (default; loopback only) | No credentials. Requests whose `Host` or `Origin` header is not loopback get `403`, so web pages cannot use the API through your browser. |
| `token = "..."` | Send `Authorization: Bearer <token>`, or `?token=<token>` where headers are not possible (browser WebSockets). Otherwise `401`. |

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
| `POST` | `/displays/{id}/brightness` | `{"percent": 0-100}` | Sets the backlight |
| `POST` | `/displays/{id}/power` | `{"on": true \| false}` | Switches the screen on or off |
| `POST` | `/displays/{id}/clear` | | Stops the current content and blanks the screen |
| `POST` | `/displays/{id}/stop` | | Stops the current content; the screen keeps its picture |
| `GET` | `/displays/{id}/stream` | | WebSocket for live frames (see below) |

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
      "submitted": 925, "shown": 908, "dropped": 17, "duplicates": 0,
      "last_encode_ms": 5.39, "last_send_ms": 21.85, "last_bytes": 50940
    }
  }
]
```

- `width` x `height` is the size to draw at (landscape). Other sizes are fitted.
- `content` is `nothing`, `image`, `clock` or `stream`. It is remembered while the display is
  unplugged (`connected: false`, `stats: null`).
- `stats`: `dropped` counts frames replaced by newer ones before they could be sent;
  `duplicates` counts frames skipped because nothing changed.

### `POST /displays/{id}/image`

The body is a PNG, JPEG, GIF or WebP file (up to 64 MiB). Query parameters:

| Parameter | Values | Default |
|---|---|---|
| `fit` | `contain` (letterbox), `cover` (crop), `stretch` | `contain` |
| `persist` | `true` also stores the image on the device (survives power loss, writes flash) | `false` |

```sh
curl --data-binary @photo.png "http://127.0.0.1:7920/api/v1/displays/default/image?fit=cover"
```

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
  image, the clock, `stop`) ends the stream: the next frame is answered with close code
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
