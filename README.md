# sub-screen-player

[![CI](https://github.com/nomunomu0504/sub-screen-player/actions/workflows/ci.yml/badge.svg)](https://github.com/nomunomu0504/sub-screen-player/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/nomunomu0504/sub-screen-player)](https://github.com/nomunomu0504/sub-screen-player/releases/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

[日本語](README.ja.md) · **Website: [subscreen.dev](https://subscreen.dev/)** (downloads, one-line installer, docs)

Drive small USB "sub screens" (the long bar displays that sit under a monitor or inside a PC
case) from macOS, Linux and Windows. A single binary, `ssp`, runs a small daemon that keeps
the display alive and shows a clock, a system dashboard with your own figures, images,
animations, videos, web pages or live frames at up to 60 fps. Anything else can draw on the
screen through the daemon's HTTP and WebSocket API.

> **Status: early.** The D92 works on macOS (Apple Silicon), and on Linux
> (Ubuntu 24.04) and Windows 11 on ARM64, checked with `ssp selftest`. The x86_64 builds for
> Linux and Windows pass CI but have not been tried with a display yet. Reports are welcome.

## Supported displays

| Display | Panel | USB id | Status |
|---|---|---|---|
| upHere D92 / MiraBox D92 (9.2") | 1920x462 | `2100:0006` (HID) | Tested on macOS, Linux and Windows (ARM64): live frames at up to 60 fps, saved images, brightness, power ([details](docs/devices/d92.md#tested-platforms)) |

Have another display? See [Adding a device](docs/adding-a-device.md). Only the
device-specific protocol has to be written; everything else is shared.

## Features

- **Live frames at up to 60 fps.** Frames are encoded on one thread and sent on another. If
  frames arrive faster than the device can take them, only the newest one is sent. Where the
  display allows it (the D92 does), only the changed parts of a frame are sent: a clock moves
  about 4 KB a second instead of 50 KB.
- **Built-in clock** with configurable formats and colors, Japanese dates included.
- **System dashboard**: the time next to CPU, memory, network and disk use, with graphs of the
  last minute.
- **Your own figures on the dashboard**: any script can send a value (a CI status, a queue
  length, the weather) with `ssp metric set` or the API, and it shows up as a panel with a graph.
- **Claude Code usage panel**: tokens used in the current 5-hour block and today, read from
  Claude Code's local logs (nothing is sent anywhere).
- **Images and animations**: PNG, JPEG, GIF and WebP, fitted to the panel (`contain`, `cover` or
  `stretch`). Animated GIF, APNG and WebP play in a loop.
  An image can optionally be stored on the device so it survives power loss.
- **Videos**: MP4, MOV, WebM, MKV and anything else ffmpeg reads, played in a loop at up to
  60 fps when [ffmpeg](https://ffmpeg.org/) is installed.
- **Web pages**: any HTML/CSS/JavaScript page or URL, drawn by headless Chrome (downloaded on
  first use) and sent whenever it repaints.
- **Notifications**: `ssp notify` shows a message over whatever is on the screen for a while, then
  goes back; Claude Code hooks and CI jobs can call it.
- **Layouts**: the clock, a video and a metric panel side by side, each zone running on its own,
  without a browser.
- **Schedules**: change what is shown, the brightness and the screen at set times of day, or show
  several screens in turn.
- **HTTP + WebSocket API** so scripts and apps in any language can draw on the screen.
- **Hotplug**: unplug and replug the display and it carries on with what it was showing.
- **Autostart** at login (launchd, systemd user unit or the Windows `Run` key).
- **Secure by default**: the API listens on localhost only and refuses requests from web
  pages. Exposing it on the network requires a token.

## What it can show

Some of the built-in screens. Settings shown as TOML go in the config file (`ssp config init`
creates it; restart the daemon after editing).

**The clock** (`ssp clock`), shown by default:

![The built-in clock](docs/images/clock.png)

**A Japanese date**, drawn with a font installed on your system:

![The clock with a Japanese date](docs/images/clock-ja.png)

```toml
[clock]
date_format = "%Y年%m月%d日（%a）"
weekdays = ["日", "月", "火", "水", "木", "金", "土"]
```

**Just the time, large and in your color:**

![The time alone in amber](docs/images/clock-big.png)

```toml
[clock]
seconds = false
date_format = ""
color = "#FFD080"
```

**The dashboard** (`ssp dashboard`): the time with CPU, memory, network and disk use, each with
a graph of the last minute:

![The dashboard](docs/images/dashboard.png)

**Fewer panels**, e.g. `ssp dashboard --widgets clock,cpu,network` (with `seconds = false` in
`[clock]`):

![The dashboard with the clock, CPU and network](docs/images/dashboard-compact.png)

**Your own colors**, without the clock:

![The dashboard in blue on dark navy](docs/images/dashboard-colors.png)

```toml
[dashboard]
widgets = ["cpu", "memory", "network", "disk"]
color = "#E8EEF8"
accent = "#60A5FA"
background = "#0B1220"
```

**Your own figures** next to the built-in ones: send them from any script with `ssp metric set`
and add `metric:<id>` panels ([how](docs/cli.md#show-your-own-figures-ci-status-a-queue-the-weather)):

![Panels for a CI status, a deployment and a queue](docs/images/dashboard-metrics.png)

```sh
ssp metric set ci --value 2 --label CI --unit failed --detail "main · 39 of 41 jobs passed"
ssp dashboard --widgets clock,metric:ci,metric:deploy,metric:queue
```

**How much Claude Code you have used**: tokens in the current 5-hour block and today, from the
logs Claude Code keeps on your computer ([details](docs/cli.md#show-how-much-claude-code-you-have-used)):

![The Claude Code panel next to the clock, CPU and memory](docs/images/dashboard-claude-code.png)

```sh
ssp dashboard --widgets clock,claude-code,cpu,memory
```

**Anything you can build as a web page**, drawn by headless Chrome
([how](docs/cli.md#show-a-web-page)):

![A web page showing the time and how much of the day has gone](docs/images/web-day.png)

```sh
ssp web contrib/web/day.html
```

**Videos**, looped at up to 60 fps through the [ffmpeg](https://ffmpeg.org/) installed on your
computer ([how](docs/cli.md#play-a-video)):

![A frame of a video zooming into the Mandelbrot set](docs/images/video.jpg)

```sh
ssp show clip.mp4 --fit cover
```

Anything else is up to you: show a picture with `ssp show`, or send frames from your own program
([below](#drawing-from-your-own-program)).

## Install

### One-line installer

```sh
curl -fsSL https://subscreen.dev/install.sh | sh              # macOS, Linux
powershell -c "irm https://subscreen.dev/install.ps1 | iex"   # Windows
```

It downloads the latest release for your system, checks its SHA-256 checksum and puts `ssp` on
your `PATH`.

### Prebuilt binaries

Download the archive for your platform from the
[latest release](https://github.com/nomunomu0504/sub-screen-player/releases/latest), extract it
and put `ssp` (`ssp.exe` on Windows) somewhere on your `PATH`.

| Platform | Archive |
|---|---|
| macOS (Apple Silicon and Intel) | `ssp-<version>-universal-apple-darwin.tar.gz` |
| Linux x86_64 / arm64 (static, any distribution) | `ssp-<version>-x86_64-unknown-linux-musl.tar.gz` / `...-aarch64-unknown-linux-musl.tar.gz` |
| Windows x64 / ARM64 | `ssp-<version>-x86_64-pc-windows-msvc.zip` / `...-aarch64-pc-windows-msvc.zip` |

`SHA256SUMS.txt` lists the checksums. The binaries are not code-signed yet:

- **macOS** may refuse to run a downloaded `ssp`. Remove the quarantine flag once:
  `xattr -d com.apple.quarantine ssp`
- **Windows** SmartScreen may warn on first start: choose "More info" → "Run anyway".

### From source

Build from source. [mise](https://mise.jdx.dev) installs the pinned Rust toolchain. On macOS
you also need the Xcode Command Line Tools (`xcode-select --install`), and on Windows the
Visual Studio C++ build tools that Rust uses anyway; they compile the bundled hidapi library.

```sh
git clone https://github.com/nomunomu0504/sub-screen-player.git
cd sub-screen-player
mise install            # the Rust version from mise.toml
mise run build          # produces target/release/ssp
```

Alternatively, with an existing Rust toolchain:
`cargo install --path crates/cli --locked`.

### Linux permissions

Allow your user to access the display, then replug it. The rule file is in the Linux archives
and in `contrib/linux/` of the source:

```sh
sudo cp contrib/linux/70-sub-screen-player.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
```

## Quick start

```sh
ssp serve                      # run the daemon (Ctrl-C stops it); shows the clock
```

In another terminal:

```sh
ssp devices                    # list displays
ssp show photo.jpg --fit cover # show an image
ssp clock --no-seconds         # back to the clock, without seconds
ssp dashboard                  # the time with CPU, memory, network and disk use
ssp brightness 60              # backlight in percent
ssp off                        # screen off (`ssp on` turns it back on)
ssp status                     # frame counters and timings
ssp selftest                   # check a display (with the daemon stopped)
ssp service install            # start the daemon automatically at login
```

With several displays, pick one with `--display <id>` (ids are shown by `ssp devices`).
The [command line guide](docs/cli.md) has every option and recipes such as dimming at night
or controlling the display from another computer.

## Configuration

`ssp config init` writes a commented config file; `ssp config path` prints where it is. Every
setting is optional:

```toml
listen = "127.0.0.1:7920"

[display]
brightness = 80          # applied when a display connects
on_exit = "leave"        # "leave", "save-last", "clear" or "sleep"

[startup]
show = "clock"           # "clock", "dashboard", "image" or "nothing"

[clock]
seconds = true
date_format = "%Y-%m-%d %a"
```

## Drawing from your own program

Stream frames over a WebSocket. Each binary message is one image (PNG, JPEG, ...) or raw
pixels:

```python
import asyncio, io, websockets
from PIL import Image, ImageDraw

async def main():
    url = "ws://127.0.0.1:7920/api/v1/displays/default/stream"
    async with websockets.connect(url) as ws:
        for n in range(600):
            img = Image.new("RGB", (1920, 462))
            ImageDraw.Draw(img).text((40, 200), f"frame {n}", fill="white")
            buf = io.BytesIO()
            img.save(buf, "JPEG")
            await ws.send(buf.getvalue())
            await asyncio.sleep(1 / 30)

asyncio.run(main())
```

Or send one image with plain HTTP:

```sh
curl --data-binary @photo.png "http://127.0.0.1:7920/api/v1/displays/default/image?fit=cover"
```

See [docs/api.md](docs/api.md) for all endpoints.

## Things to know

- `ssp show --persist` and `on_exit = "save-last"` write the image to the device's flash
  memory. That is fine now and then, but do not do it on every frame.
- The D92 restarts itself about 8 seconds after the daemon stops sending keep-alives. It then
  shows the last image that was stored on it.
- The D92 vendor app has an "extended screen" mode (`SCREEN`) that leaves the display
  unresponsive until it is unplugged. This project never sends it.

## Documentation

Every document is available in English and Japanese (switch with the link at the top of each page).

- [Command line guide](docs/cli.md): use cases and every command
- [Architecture](docs/architecture.md): how the pieces fit together and where code belongs
- [Adding a device](docs/adding-a-device.md)
- [HTTP / WebSocket API](docs/api.md)
- [D92 protocol notes](docs/devices/d92.md)
- [Contributing](CONTRIBUTING.md)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. The embedded Go font is under its own BSD-style
license ([crates/server/assets/fonts/LICENSE-Go-fonts.txt](crates/server/assets/fonts/LICENSE-Go-fonts.txt)).

This project is not affiliated with or endorsed by upHere, MiraBox or any other display
vendor. Device protocols were worked out for interoperability.
