# sub-screen-player

[![CI](https://github.com/nomunomu0504/sub-screen-player/actions/workflows/ci.yml/badge.svg)](https://github.com/nomunomu0504/sub-screen-player/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/nomunomu0504/sub-screen-player)](https://github.com/nomunomu0504/sub-screen-player/releases/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

[日本語](README.ja.md) · **Website: [nomunomu0504.github.io/sub-screen-player](https://nomunomu0504.github.io/sub-screen-player/)** (downloads, one-line installer, docs)

Drive small USB "sub screens" (the long bar displays that sit under a monitor or inside a PC
case) from macOS, Linux and Windows. A single binary, `ssp`, runs a small daemon that keeps
the display alive and shows a clock, images or live frames at up to 60 fps. Anything else can
draw on the screen through the daemon's HTTP and WebSocket API.

> **Status: early (v0.1).** The D92 works on macOS (Apple Silicon), and on Linux
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
  frames arrive faster than the device can take them, only the newest one is sent.
- **Built-in clock** with configurable formats and colors.
- **Images**: PNG, JPEG, GIF and WebP, fitted to the panel (`contain`, `cover` or `stretch`).
  An image can optionally be stored on the device so it survives power loss.
- **HTTP + WebSocket API** so scripts and apps in any language can draw on the screen.
- **Hotplug**: unplug and replug the display and it carries on with what it was showing.
- **Autostart** at login (launchd, systemd user unit or the Windows `Run` key).
- **Secure by default**: the API listens on localhost only and refuses requests from web
  pages. Exposing it on the network requires a token.

## Install

### One-line installer

```sh
curl -fsSL https://nomunomu0504.github.io/sub-screen-player/install.sh | sh   # macOS, Linux
irm https://nomunomu0504.github.io/sub-screen-player/install.ps1 | iex       # Windows (PowerShell)
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
show = "clock"           # "clock", "image" or "nothing"

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
