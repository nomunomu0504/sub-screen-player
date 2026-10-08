# Architecture

[日本語](architecture.ja.md)

This document explains how sub-screen-player is put together, what each part is responsible
for, and where new code belongs.

## Overview

```text
  ssp CLI        scripts / apps        (future) plugins
     │                 │                          │
     └──── HTTP / WebSocket (127.0.0.1:7920) ─────┘
                       │
┌──────────────────────┼──────────────────────────────────────── ssp-server ──┐
│  api/        routes, auth, WebSocket streams                                 │
│  manager     finds displays, hotplug, what each display shows (Content)      │
│  sources/    built-in screens that draw frames (clock, dashboard, image,     │
│              animation, video through ffmpeg, web page through Chrome)       │
│  metrics     figures sent by scripts for the dashboard's metric panels       │
└──────────────────────┼───────────────────────────────────────────────────────┘
                       │ Frame (landscape RGB, panel size)
┌──────────────────────┼──────────────────────────────────────────── ssp-core ──┐
│  Presenter   latest-frame-wins queue, encoder thread, device thread,           │
│              frame pacing, duplicate skipping, keep-alives                     │
│  Encoder     rotate to the panel's orientation, encode (JPEG)                  │
│  Display     trait every driver implements                                     │
│  Transport   trait for the byte channel; hid::HidTransport implements it       │
└──────────────────────┼─────────────────────────────────────────────────────────┘
                       │ EncodedImage
┌──────────────────────┼─────────────────────────────── ssp-driver-<model> ──────┐
│  protocol    pure functions that build the device's reports                    │
│  Display     implementation: show, save, brightness, power, keep-alive         │
│  Driver      USB ids it handles, how to open the device                        │
└──────────────────────┼─────────────────────────────────────────────────────────┘
                       │ reports
                    USB (HID)
```

The `ssp` binary contains all of it: `ssp serve` runs the daemon, and every other command is a
small HTTP client of that daemon.

## Crates and responsibilities

| Crate | Path | Responsible for | Must not |
|---|---|---|---|
| `ssp-core` | `crates/core` | The `Display`/`Driver`/`Transport` traits, HID access, frames and encoding, the `Presenter` | Know any device's protocol |
| `ssp-driver-<model>` | `crates/drivers/<model>` | One family of devices: its protocol, capabilities and quirks | Depend on other drivers, the server or async code |
| `ssp-server` | `crates/server` | The daemon: device management, built-in sources, config, HTTP/WebSocket API | Contain device-specific bytes |
| `sub-screen-player` (bin `ssp`) | `crates/cli` | Command line, autostart, logging setup | Talk to devices directly (except listing them when no daemon runs) |

Dependencies only point downwards: `cli` → `server` → `drivers/*` → `core`.

## Key types

- **`Frame`** (core): what callers draw. Always landscape and exactly the panel's size,
  8-bit RGB. Images of other sizes are fitted with `Frame::fit` (`contain`, `cover`,
  `stretch`).
- **`PanelSpec`** (core): the panel's size as callers see it, the rotation needed on the wire
  and the image format. For example, the D92 is 1920x462, turned 90° clockwise, JPEG.
- **`EncodedImage`** (core): a frame after rotation and encoding, ready for a driver.
- **`Display`** (core trait, implemented by drivers): `show` (live, not stored), `save`
  (stored on the device), `set_brightness`, `wake`, `sleep`, `clear`, `keep_alive`.
  Operations a model lacks keep the default implementation, which returns
  `Error::Unsupported`.
- **`Capabilities`** (core): what a display supports, its frame-rate limit, keep-alive interval
  and largest accepted image. The rest of the system reads these instead of checking models.
- **`Driver`** (core trait): the USB interfaces a driver handles (`UsbMatch`) and `open`.
  The daemon's driver list is `crates/server/src/drivers.rs`. Which drivers are in use is a
  `DriverSelection` (`--driver` options, `[drivers]` in the config) applied with
  `Registry::select`; drivers marked `experimental` are only used when named.
- **`Content`** (server): what a display is told to show: nothing, an image, the clock, the
  dashboard or a stream. It is kept per display id, so a replugged display carries on.
- **`Source`** (server trait): something that draws frames and says when the picture changes
  next (the clock, the dashboard, a still image).
- **`Metrics`** (server): the figures scripts send with `PUT /api/v1/metrics/{id}`, kept in
  memory and shared by the API and every dashboard. A `metric:<id>` panel reads its metric each
  time it draws, so a new value shows within a second without telling the dashboards.

## Life of a frame

1. A frame is produced: a `Source` renders it on the display's player thread, or a
   WebSocket/HTTP client sends one and the server decodes it.
2. `Presenter::submit` stores it in a one-frame slot and returns at once. A frame still
   waiting in the slot is dropped (counted as `dropped`).
3. The **encoder thread** takes the frame, rotates it and encodes it. If the result is larger
   than the device accepts, it lowers the JPEG quality step by step. While frames are being
   dropped because the device falls behind, the device thread lowers the quality used for the
   next frames (down to `min_quality`), so smaller frames let it keep up; it raises the quality
   again once nothing is dropped, and at once after a pause (a clock, a dashboard).
4. The **device thread** sends the newest encoded frame once the frame interval
   (`1 / max_fps`) has passed. A frame identical to the one on screen is skipped (counted as
   `duplicates`). Encoding the next frame overlaps with sending this one, so the slower of
   the two sets the frame rate, not their sum.
5. The driver's `Display::show` turns the image into reports (`protocol.rs`) and writes them
   through its `Transport`.

Control commands (brightness, power, clear, save) go through the same device thread, in
order, and the caller waits for the result. Keep-alives are sent on that thread every
`keep_alive_interval`.

## Threads

| Thread | Count | Job |
|---|---|---|
| tokio runtime | a few | HTTP and WebSocket handling. Blocking work goes to `spawn_blocking`. |
| `ssp-scan` | 1 | Scans USB every 2 s, opens new displays and notices unplugged ones |
| `ssp-encode-<id>` | 1 per display | Rotates and encodes frames |
| `ssp-device-<id>` | 1 per display | Owns the `Display`; sends frames, commands and keep-alives |
| `ssp-player-<id>` | 0–1 per display | Runs the current built-in `Source` (e.g. ticks the clock) |

A `Display` is only ever used by its device thread, so drivers need no locking.

## Errors and reconnection

- `Error::Transport` and `Error::Disconnected` are **fatal**: the presenter stops and fails
  queued commands. On its next scan the manager drops the device, keeps its `Content`, and
  reopens it when it shows up again.
- Other errors (`InvalidArgument`, `Image`, `Unsupported`) are returned to the caller and the
  display keeps running.
- When a device fails to open (e.g. another program holds it), the manager retries every 10 s
  and logs the first failure only.

## Security model

- The API listens on `127.0.0.1` by default. The config refuses a non-loopback `listen`
  address unless a `token` is set.
- Without a token, requests whose `Host` or `Origin` header is not loopback are rejected.
  This stops web pages in the user's browser from driving the display (CSRF, DNS rebinding).
- With a token, every request needs `Authorization: Bearer <token>` or `?token=<token>`.
- The daemon reads no files of the user's beyond its config and the images it is told to show,
  except Claude Code's session logs for the `claude-code` panel, and only once a dashboard shows
  that panel. Only token counts and times are kept, in memory; they are visible to API clients as
  the metric `claude-code`.
- Web pages run in headless Chrome with a new, empty profile, deleted when the page is replaced.
  Any API client can make the daemon open a URL, `file://` included, so a daemon reachable from
  the network (with a token) lets token holders show the computer's local files on its display.
- Headless Chrome is downloaded over HTTPS from Google's Chrome for Testing storage, only when the
  user asks (`ssp web` confirms first) or sets `[web] auto_download`. Chrome for Testing publishes
  no checksums; the zip's own CRCs are checked while unpacking.

## Extension points

What exists today and where planned features fit:

| Extension | Status | Where it fits |
|---|---|---|
| New display models | available | A new crate under `crates/drivers/` ([guide](adding-a-device.md)) |
| External programs drawing frames | available | WebSocket stream or HTTP image endpoint ([API](api.md)) |
| Built-in screens | available | A `Source` in `crates/server/src/sources/` plus a `Content` variant |
| Built-in dashboard panels with their own data | available | A collector registered with `Metrics::provide` that keeps a metric up to date (`claude_code.rs`), plus a `Widget` variant |
| Non-HID transports (USB bulk, serial) | planned | A new `Transport` implementation in `crates/core` |
| Web (HTML/CSS) screens | available | `sources/web.rs` shows a page through headless Chrome (`web/`: download, DevTools protocol) |
| WASM plugins | planned | A `Source` that hosts a sandboxed plugin |
| Layouts composed in the config | planned | A `Source` that combines other sources, configured in `config.rs` |

The `Source` trait and the stream API are the two seams new kinds of content plug into. Keep
them small and stable.
