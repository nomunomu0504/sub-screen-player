# Command line guide

[日本語](cli.ja.md)

Everything is done with one command, `ssp`. `ssp serve` runs the daemon that owns the
displays; every other command asks that daemon to do something, over its
[HTTP API](api.md).

- [Use cases](#use-cases): recipes for common tasks
- [Command reference](#command-reference): every command and option
- [Troubleshooting](#troubleshooting)

## Use cases

### Show a clock whenever you are logged in

```sh
ssp service install
```

The daemon now starts at login and shows the clock on every display it finds. To change the
clock's look permanently, create a config file and edit its `[clock]` section:

```sh
ssp config init        # writes a commented config file and prints its path
```

```toml
[clock]
seconds = false
date_format = "%a %-d %b"     # e.g. "Wed 7 Oct"; "" hides the date
color = "#FFD080"
```

Then [restart the daemon](#autostart) so it reads the file.

For dates in another language, list the weekday names from Sunday in `weekdays`; `%a` (and `%A`)
then print those names. Characters the built-in font lacks (e.g. Japanese) are drawn with a font
installed on the system (Hiragino on macOS, Yu Gothic or Meiryo on Windows, Noto Sans CJK and similar
on Linux). If Japanese does not show on Linux, install a package such as `fonts-noto-cjk`.

```toml
[clock]
date_format = "%Y年%m月%d日（%a）"     # e.g. "2026年10月08日（木）"
weekdays = ["日", "月", "火", "水", "木", "金", "土"]
```

To change the clock just for now, use options instead:

```sh
ssp clock --no-seconds --date-format ""
```

### Show CPU, memory and network use next to the time

```sh
ssp dashboard
```

The dashboard shows the time (formatted as in `[clock]`) next to panels for CPU, memory, network
and disk use. CPU, memory and network come with a graph of the last minute. The network panel
shows the download speed with the upload speed below it (loopback is not counted), and the disk
panel the space used on the system disk (`/`, or `C:\` on Windows). Pick and order the panels with
`--widgets`:

```sh
ssp dashboard --widgets clock,cpu,network
```

To show the dashboard at login instead of the clock, and to keep your panels and colors, edit the
config file and [restart the daemon](#autostart):

```toml
[startup]
show = "dashboard"

[dashboard]
widgets = ["clock", "cpu", "memory", "network"]
accent = "#FFD080"       # color of the graphs
```

### Show your own figures: CI status, a queue, the weather

Any script can put a figure on the dashboard. Send it with `ssp metric set` (or
[the API](api.md#metrics)) and add a `metric:<id>` panel:

```sh
ssp metric set ci --value 2 --label CI --unit failed --detail "main · 39 of 41 jobs passed"
ssp metric set deploy --text live --label Deploy --detail "v0.3.0"
ssp dashboard --widgets clock,metric:ci,metric:deploy,cpu
```

![Panels for a CI status, a deployment and a queue](images/dashboard-metrics.png)

Each `--value` is also added to the panel's graph. Send a new value whenever it changes, e.g. from
cron or a CI job; `--value -` reads the number from standard input:

```sh
uptime | awk '{print $(NF-2)}' | tr -d , | ssp metric set load --value - --label "Load"
```

A value older than `--ttl` seconds (default 300) is dimmed and says how old it is, so a stopped
script does not leave a figure that looks current. Metrics are kept in memory only: after the daemon
restarts, panels say "waiting for data" until the next value arrives.
[contrib/metrics/github-ci.sh](../contrib/metrics/github-ci.sh) is a complete example that shows
how the last GitHub Actions runs of a branch went.

### Show how much Claude Code you have used

```sh
ssp dashboard --widgets clock,claude-code,cpu,memory
```

![The Claude Code panel next to the clock, CPU and memory](images/dashboard-claude-code.png)

The `claude-code` panel adds up the tokens of every Claude Code session on this computer
(subagents included):

- the large figure is the current 5-hour block, with the time it ends: a block starts at the
  hour of the first reply after the previous block ended and lasts five hours, which is how
  Claude's usage limits are usually tracked. It is an estimate from your logs, not the limit
  Anthropic counts;
- `today` is the total since midnight;
- the graph shows tokens per minute over the last hour.

Input, output and cache-creation tokens are counted. Cache reads (the conversation read again on
every turn, billed at a fraction) are not, or they would dwarf the rest.

**What is read**: the session logs Claude Code keeps in `~/.claude/projects/` (or under
`$CLAUDE_CONFIG_DIR`, or `~/.config/claude`), of which only the token counts and times of the
replies are used. Nothing is sent anywhere, and nothing is read until a dashboard shows the
panel. The first read of a day's logs takes about a second; after that, only new lines are read,
every 30 seconds. Point it elsewhere with `[claude_code] dir` in the config. The same figures are
available to scripts as the metric `claude-code` (`ssp metric list`,
[`GET /api/v1/metrics/claude-code`](api.md#metrics)).

### Get notified: Claude Code waiting, a failed CI run

```sh
ssp notify "Build finished" --color green
ssp notify "CI failed" --detail "main · 2 of 41 jobs" --color red --sticky
ssp notify --dismiss
```

![A notification over the clock](images/notify.png)

`ssp notify` draws a message over whatever the display shows, for 10 seconds (`--for
<seconds>`, up to a day) or until it is dismissed or replaced (`--sticky`); then the screen goes
back to what it showed. The content keeps running underneath: the clock keeps ticking above the
banner, a video keeps playing. A new notification replaces the current one. `--detail` adds a
smaller line, `--color` sets the background (`red`, `orange`, `yellow`, `green`, `blue`, `gray`
or `#rrggbb`), and `--style full` covers the whole panel instead of the bottom third. A screen
that is off stays off unless `--wake` is given; then it goes off again when the notification
ends. The notification stays when the content changes and while the display is unplugged;
`ssp status` shows it with the time left, and `ssp clear` ends it.

**Claude Code** runs hooks when it needs you and when it finishes. Add them to
`~/.claude/settings.json`:

```json
{
  "hooks": {
    "Notification": [
      { "hooks": [{ "type": "command", "command": "ssp notify --stdin --for 60" }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "ssp notify \"Claude Code is done\" --stdin --color green" }] }
    ]
  }
}
```

With `--stdin`, `ssp notify` reads what the hook gets: the message ("Claude needs your permission
to use Bash") becomes the text, and the project folder and the first line of Claude's last
reply the detail. Plain text works too: the first line is the text, the rest the detail.

**CI or any script**: run `ssp notify` when a job fails, or send `POST /api/v1/displays/{id}/notify`
from another computer (see the [API guide](api.md)).

### Keep a picture on the screen, even when the computer is off

```sh
ssp show wallpaper.png --fit cover --persist
```

`--persist` also stores the image on the display, so it comes back after the computer is shut
down or the display is replugged. Without it, the picture is only shown while the daemon runs.

Storing writes the display's flash memory and takes a moment (about 1.5 s on the D92). Use it
for pictures you keep for a while, not for animations or things that change every minute.

To have the last picture stored automatically whenever the daemon exits, set this in the
config file:

```toml
[display]
on_exit = "save-last"
```

### Play a video

```sh
ssp show clip.mp4 --fit cover
```

Videos play in a loop, at their own frame rate up to 60 fps, through the
[ffmpeg](https://ffmpeg.org/) installed on your computer (`ssp` has no video decoder of its
own). Install it with `brew install ffmpeg` (macOS), `sudo apt install ffmpeg` (Debian, Ubuntu)
or `winget install ffmpeg` (Windows); if the daemon cannot find it (a service started at login
may not see your `PATH`), set its path in the config:

```toml
[video]
ffmpeg = "/opt/homebrew/bin/ffmpeg"
```

Sound is ignored. Decoding a 720p video at 30 fps costs ffmpeg about a fifth of a CPU core on an Apple Silicon Mac.
How smooth it looks depends on the display: the D92 takes about 2.2 MB/s, so a detailed
full-screen video runs at about 40 fps, a calmer one at 60. To play a video whenever the daemon
starts, set `show = "image"` and `image = "/path/to/clip.mp4"` in `[startup]`.

### Show a web page

```sh
ssp web contrib/web/day.html
ssp web https://example.com/status --reload 600
```

![A web page showing the time and how much of the day has gone](images/web-day.png)

Anything you can build with HTML, CSS and JavaScript can be a screen: a page is drawn by headless
Chrome at the panel's size (1920x462 on the D92) and sent to the display whenever it repaints, so
CSS animations and pages that update themselves run as they would in a browser, at up to 60 fps.
[contrib/web/day.html](../contrib/web/day.html) is a small example to start from. `--reload`
reloads the page every so many seconds, for pages that do not update themselves. A file is passed
to the daemon as a `file://` URL, so with a daemon on another computer it has to be there.

A page can also read the daemon's figures and draw its own dashboard: your metrics, the Claude
Code usage, and the CPU, memory, network and disk figures. The daemon gives each page it shows
a token that only reads (`window.ssp`); see "Pages shown with `ssp web`" in the
[API guide](api.md). [contrib/web/system.html](../contrib/web/system.html) shows how.

**Headless Chrome** is not part of `ssp`. The first `ssp web` asks to download it (about 100 MB,
from Google's [Chrome for Testing](https://googlechromelabs.github.io/chrome-for-testing/)) into
`~/Library/Application Support/sub-screen-player/chrome` (macOS),
`~/.local/share/sub-screen-player/chrome` (Linux) or
`%LOCALAPPDATA%\sub-screen-player\data\chrome` (Windows). `ssp web --install` downloads it
without showing anything, and again later to update it; `--yes` skips the question. To use an
installed Chrome, Chromium or Edge instead, set it in the config:

```toml
[web]
chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
```

The browser runs only while a page is shown and uses a fresh, empty profile each time: no
cookies, logins or extensions of your own browser. JavaScript is on, and a page can load whatever
it links to, so show pages you trust. A browser costs 300-400 MB of memory, and about half a CPU
core while a page animates at 60 fps; a still page costs almost nothing after it is drawn. If a page
cannot be shown (no network, a missing file, a browser that cannot start), the display says why.
On Linux, a downloaded Chrome needs the usual browser libraries; if it does not start, install
`chromium` from your distribution and set `[web] chrome` to it. To show a page whenever the daemon
starts:

```toml
[startup]
show = "web"
url = "file:///home/me/panel.html"
reload = 600       # optional
```

### Stop, blank or turn off the screen

| Command | What happens to the picture | The screen | The clock / dashboard / image / stream |
|---|---|---|---|
| `ssp stop` | stays as it is | on | stopped |
| `ssp clear` | becomes black | on | stopped |
| `ssp off` | is kept | off (backlight off) | keeps running |
| `ssp on` | | on again | |

Start something again with `ssp clock` or `ssp show ...`. A notification (`ssp notify`) stays
over `ssp stop` and `ssp off`; `ssp clear` ends it.

### Dim the screen at night

Schedule `ssp brightness`. With cron (macOS and Linux, `crontab -e`):

```text
0 22 * * * /usr/local/bin/ssp brightness 20
0 7  * * * /usr/local/bin/ssp brightness 100
```

On Windows:

```bat
schtasks /Create /SC DAILY /ST 22:00 /TN "ssp dim" /TR "C:\Tools\ssp.exe brightness 20"
schtasks /Create /SC DAILY /ST 07:00 /TN "ssp bright" /TR "C:\Tools\ssp.exe brightness 100"
```

Use the full path of `ssp`, since scheduled jobs run with a minimal `PATH`. To set the
brightness every time a display connects, use `brightness = 80` in the `[display]` section of
the config file.

### Several displays

```sh
ssp devices
```

```text
ID                MODEL       SIZE      CONTENT  STATE
d92-470B03781D1F  upHere D92  1920x462  clock    connected
d92-5C2A11FE0A42  upHere D92  1920x462  clock    connected
```

Pick a display with `-d` / `--display`:

```sh
ssp -d d92-470B03781D1F clock
ssp -d d92-5C2A11FE0A42 show photo.jpg
```

Without `--display`, commands go to `default`: the first connected display in id order.
Each display remembers what it shows, also when it is unplugged and plugged in again.

### Control the display from another computer

By default the daemon only accepts connections from the same computer. To allow others, set
an address and a token in the config file of the computer the display is attached to:

```toml
listen = "0.0.0.0:7920"
token = "a-long-random-string-of-at-least-16-characters"
```

Then, on the other computer:

```sh
export SSP_URL=http://192.168.1.10:7920
export SSP_TOKEN=a-long-random-string-of-at-least-16-characters
ssp status
ssp show photo.jpg
```

The connection is plain HTTP, so the token and images are not encrypted. Only do this on a
network you trust.

### Show your own content (dashboards, games, ...)

Any program can stream frames to the daemon over a WebSocket, up to 60 frames per second. See
the example in the [README](../README.md#drawing-from-your-own-program) and the
[API reference](api.md#websocket-stream). While a stream runs, `ssp devices` shows `stream` as
the content. `ssp clock` or `ssp show` takes the display back.

Detailed pictures at a high frame rate can be more than the USB link carries: on the D92 a
detailed 1920x462 frame at JPEG quality 85 takes about 22 ms to send, so about 45 frames per
second get through. While frames come faster than the display takes them, the JPEG quality is
lowered (down to `min_quality`, 70 by default) until it keeps up, and raised again as soon as
it does; `ssp status` shows the quality in use. A clock or a dashboard stays at full quality.
To always keep the full quality, set `min_quality` to the same value as `quality`:

```toml
[display]
quality = 85
min_quality = 85       # never lower the quality, drop frames instead
```

Displays that can draw a picture over a part of the screen (the D92 can) get only the parts of
a frame that changed: a clock sends its seconds as about 4 KB instead of a 50 KB frame, and a
web page with a small animation only the moving part. `ssp status` shows how many frames went
in parts. Whole frames still go out every 10 seconds, after any command, and when more than
half of the screen changes (videos, most animations). To always send whole frames:

```toml
[display]
partial_updates = false
```

### Develop support for a new display while another one keeps running

Say a D92 shows your clock and you are writing a driver `dxxxx` for another display. Run the
daemon with only the drivers it should use, and test the new display next to it:

```sh
ssp serve --driver d92            # the daemon leaves every other display alone
ssp selftest --driver dxxxx       # in another terminal, as often as you like
ssp devices --driver dxxxx        # is the new display recognized?
```

A driver that is still being developed can be marked experimental (see
[Adding a device](adding-a-device.md)); it is then only used when named with `--driver` or in
`[drivers] enable`, so it never takes over a display by accident.

### Run the daemon by hand

```sh
ssp serve                                # logs to the terminal, Ctrl-C stops it
SSP_LOG=debug ssp serve                  # more detail
ssp serve --listen 127.0.0.1:8000        # another port (then use --url for other commands)
ssp serve --log-file ~/ssp.log           # log to a file
```

## Command reference

### Global options

These work with every command.

| Option | Environment variable | Default | Meaning |
|---|---|---|---|
| `--config <FILE>` | `SSP_CONFIG` | see [below](#config-file-location) | Config file to use |
| `--url <URL>` | `SSP_URL` | from `listen` in the config (`http://127.0.0.1:7920`) | Where the daemon is |
| `--token <TOKEN>` | `SSP_TOKEN` | `token` in the config | API token, if the daemon requires one |
| `-d`, `--display <ID>` | | `default` | Display to act on (see `ssp devices`) |
| `-h`, `--help` | | | Help for a command, e.g. `ssp show --help` |
| `-V`, `--version` | | | Print the version |

`SSP_LOG` sets the daemon's log level for `ssp serve` (`info` by default; e.g. `debug`,
`ssp_server=debug`).

### Daemon

| Command | Description |
|---|---|
| `ssp serve` | Run the daemon in the foreground. Stops on Ctrl-C (or SIGTERM), then applies `on_exit` from the config. |
| `  --listen <ADDR>` | Address to listen on, e.g. `127.0.0.1:8000`. Overrides `listen` in the config. |
| `  --log-file <FILE>` | Append logs to a file instead of printing them. |
| `  --driver <ID>` | Use only this driver (repeatable), e.g. `--driver d92`. Overrides `enable` in the `[drivers]` section of the config. The daemon leaves displays of other drivers alone. |

### Displays

| Command | Description |
|---|---|
| `ssp devices` | List displays: id, model, size, what they show and whether they are connected. If the daemon is not running, lists the supported displays plugged into this computer instead. |
| `  --json` | Print the full details as JSON (same as `GET /api/v1/displays`). |
| `  --driver <ID>` | Only list displays of this driver (repeatable). |
| `ssp status` | Daemon version and, per display, firmware and frame counters: frames shown, dropped (replaced by newer ones), unchanged (skipped) and received, plus the last encode time, send time and size, and the JPEG quality in use. |
| `ssp selftest` | Check connected displays directly, without the daemon. Runs about 25 s per display and reports PASS / WARN / FAIL per check: open (and firmware), commands (wake, brightness 100%), still image, partial updates (whether a small change is sent as a part, on displays that take parts), streaming (frames per second), keep-alive (still connected after being idle) and power (off and on). Several displays are all opened first and tested one after another. If the daemon is running, displays of the drivers it uses are skipped (it could take them at any moment): stop it, or run it with `--driver` for other drivers only. Exits with 1 if a check fails or nothing could be tested. Use `--display` to test one display. |
| `  --driver <ID>` | Only test displays of this driver (repeatable). Also enables experimental drivers. |
| `  --frames <N>` | Frames sent in the streaming check (default 180). |
| `  --hold <SECONDS>` | How long to stay idle in the keep-alive check (default 15). |
| `  --json` | Print the report as JSON. |

### What to show

| Command | Description |
|---|---|
| `ssp show <FILE>` | Show a PNG, JPEG, GIF or WebP image. Animated GIF, APNG and WebP play in a loop at their own speed. A video (MP4, MOV, WebM, MKV, ...) plays in a loop when ffmpeg is installed. |
| `  --fit contain` | (default) Fit the whole image and fill the rest with black. |
| `  --fit cover` | Fill the screen and cut off what sticks out. |
| `  --fit stretch` | Fill the screen, distorting the image if needed. |
| `  --persist` | Also store the image on the display so it survives power loss (writes flash memory). For an animation, the first frame is stored. |
| `ssp clock` | Show the built-in clock. Options not given come from `[clock]` in the config. |
| `  --no-seconds` | Hide the seconds. |
| `  --format <FMT>` | Format of the time, e.g. `"%H:%M"` or `"%I:%M %p"` ([strftime syntax](https://docs.rs/jiff/latest/jiff/fmt/strtime/)). |
| `  --date-format <FMT>` | Format of the date line, e.g. `"%Y-%m-%d %a"`. `""` hides it. |
| `  --weekdays <NAMES>` | Names printed by `%a` and `%A`: seven, comma-separated, from Sunday, e.g. `日,月,火,水,木,金,土`. |
| `ssp dashboard` | Show the built-in dashboard: the time, CPU, memory, network, disk and your own metrics. Options not given come from `[dashboard]` in the config. |
| `  --widgets <LIST>` | Panels from left to right, comma-separated: `clock`, `cpu`, `memory`, `network`, `disk`, `claude-code`, `metric:<id>`. |
| `ssp metric set <ID>` | Create or update a metric for `metric:<ID>` panels. The id is 1-32 of `a-z`, `0-9` and `-`. Options not given keep their previous value. |
| `  --value <N>` | The number shown, also added to the graph. `-` reads it from standard input. |
| `  --text <TEXT>` | A short text shown instead of a number, e.g. `passing`. |
| `  --label`, `--unit`, `--detail` | Name above the value (default: the id), unit after it, and the line under it. |
| `  --max <N>` | Top of the graph (default: the largest value in it). |
| `  --ttl <SECONDS>` | How long the value counts as current (default 300). |
| `  --series <LIST>` | Replace the graph with these values, oldest first, comma-separated. |
| `ssp metric list` | List the metrics, with their age. `--json` prints them as JSON. |
| `ssp metric rm <ID>` | Remove a metric. |
| `ssp web <URL or FILE>` | Show a web page or a local HTML file, drawn by headless Chrome. Asks to download headless Chrome the first time. |
| `  --reload <SECONDS>` | Reload the page every so many seconds. |
| `  --yes`, `-y` | Download headless Chrome without asking, if needed. |
| `  --install` | Only download (or update) headless Chrome. |
| `ssp stop` | Stop the clock, dashboard, image or stream. The screen keeps its last picture. |
| `ssp clear` | Stop the clock, dashboard, image or stream and blank the screen. |

### Screen

| Command | Description |
|---|---|
| `ssp brightness <0-100>` | Set the backlight in percent. |
| `ssp on` | Switch the screen on. |
| `ssp off` | Switch the screen off. What is being shown keeps running. |
| `ssp notify <TEXT>` | Show a message over the screen for a while, then go back to what was shown. |
| `  --detail <TEXT>` | A smaller line under the message. |
| `  --for <SECONDS>` | How long it stays: 1 to 86400, default 10. |
| `  --sticky` | Keep it until dismissed or replaced. |
| `  --style banner\|full` | Draw it across the bottom third (default) or the whole panel. |
| `  --color <COLOR>` | `red`, `orange`, `yellow`, `green`, `blue` (default), `gray` or `#rrggbb`. |
| `  --wake` | Switch the screen on if it is off, and off again after. |
| `  --stdin` | Read the message from standard input: text, or the JSON of a Claude Code hook. |
| `ssp notify --dismiss` | End the notification. |

### Autostart

| Command | macOS | Linux | Windows |
|---|---|---|---|
| `ssp service install` | LaunchAgent `~/Library/LaunchAgents/dev.sub-screen-player.ssp.plist` | systemd user unit `~/.config/systemd/user/sub-screen-player.service` | Value `sub-screen-player` in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` |
| `ssp service uninstall` | Stops the daemon and removes the agent | Stops the daemon and removes the unit | Removes the entry (a running daemon keeps running until you sign out) |
| `ssp service status` | Installed? Running? | Installed? Active? | Installed? |
| Restart (e.g. after editing the config) | `ssp service install` again | `systemctl --user restart sub-screen-player` | End `ssp.exe` in Task Manager, then `ssp service install` |
| Logs | `~/Library/Logs/sub-screen-player.log` | `journalctl --user -u sub-screen-player` | `%LOCALAPPDATA%\sub-screen-player\ssp.log` |

`install` starts the daemon right away. The entry records the path of the `ssp` executable and
of the config file at the time of installing, so run `install` again after moving either.

### Config file

| Command | Description |
|---|---|
| `ssp config path` | Print the path of the config file. |
| `ssp config init` | Write a commented config file with the default values. |
| `  --force` | Overwrite an existing file. |
| `ssp config show` | Print the effective configuration (defaults plus the file). |

#### Config file location

| OS | Path |
|---|---|
| macOS | `~/Library/Application Support/sub-screen-player/config.toml` |
| Linux | `~/.config/sub-screen-player/config.toml` (`$XDG_CONFIG_HOME` is respected) |
| Windows | `%APPDATA%\sub-screen-player\config\config.toml` |

The file is optional; without it the defaults apply. `ssp config init` shows every setting
with a comment. The daemon reads the file when it starts.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | The command failed, e.g. the daemon is not running or refused the request. The reason is printed after `error:`. |
| `2` | Invalid command line arguments |

## Troubleshooting

1. **`ssp selftest`** checks the whole path from USB to the screen without the daemon. Stop the
   daemon, run it, and include its output when you [report a problem](../CONTRIBUTING.md).
2. **`ssp devices`**
   - "The daemon is not running" and your display is listed as plugged in: start the daemon
     with `ssp serve` or `ssp service install`.
   - Your display is not listed at all: check the cable. On Linux, install the udev rule (see
     the [README](../README.md#install)) and replug the display.
3. **The daemon log says "cannot open ... display"**: another program is using the display,
   for example the vendor's app or a second `ssp serve`. Close it; the daemon retries every
   10 seconds.
4. **"cannot listen on 127.0.0.1:7920 (is the daemon already running?)"**: a daemon is already
   running (`ssp service status`), or another program uses the port. Use `--listen` with
   another port, and `--url` for the other commands.
5. **`ssp status`** shows whether frames reach the display. A growing `shown` count means
   they do. Many `dropped` frames are normal when a program streams faster than the display
   can take.
6. **More detail**: run `SSP_LOG=debug ssp serve` and watch the log.
7. **The D92 shows an old picture after the daemon stopped**: the D92 restarts about 8 seconds
   after the daemon stops talking to it and then shows the last *stored* image. Store the one
   you want with `ssp show --persist`, or set `on_exit = "save-last"`.
8. **The display does not react to anything**: unplug it and plug it in again. The daemon
   picks it up again on its own.
