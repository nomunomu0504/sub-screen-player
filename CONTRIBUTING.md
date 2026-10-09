# Contributing to sub-screen-player

[日本語](CONTRIBUTING.ja.md)

Thanks for helping! Bug reports, support for new displays, features and documentation are all
welcome. This guide explains how to set up the project, where code belongs and what a pull
request needs.

## Ways to contribute

- **Report a bug** with the issue form. Include your OS, the display model, the output of
  `ssp selftest` (run it with the daemon stopped), `ssp devices` and `ssp status`, and the
  daemon's log (run `SSP_LOG=debug ssp serve`).
- **Report a security problem** privately as described in [SECURITY.md](SECURITY.md), not in
  a public issue.
- **Add a display.** Follow [docs/adding-a-device.md](docs/adding-a-device.md). If you only
  have notes on how a display works, open an issue with them; someone else may write the driver.
- **Add a screen to the [gallery](https://subscreen.dev/screens/).** Write a page for
  1920x462 in `site/public/screens/<name>/index.html` that reads the figures from `window.ssp`
  and shows example figures without it, so it can be tried in a browser (a screen that reads the
  internet shows them with `?example`). Draw its picture with
  `sh site/scripts/shoot-screens.sh <name>` and add it to `site/src/components/gallery/screens.ts`.
- **Improve features or docs.** For anything larger than a small fix, open an issue first so
  we can agree on the approach before you invest time.

## Development setup

Tool versions are pinned in [`mise.toml`](mise.toml), so everyone builds with the same Rust.

```sh
mise install        # installs the pinned Rust toolchain with rustfmt and clippy
mise run ci         # what CI runs: format check, clippy, tests
```

| Task | What it does |
|---|---|
| `mise run fmt` | Format the code |
| `mise run lint` | `cargo clippy` with warnings as errors |
| `mise run test` | Run all tests (no hardware needed) |
| `mise run build` | Release build (`target/release/ssp`) |
| `mise run serve` | Run the daemon from source |

The website (`site/`, built with [Astro Starlight](https://starlight.astro.build) and
[bun](https://bun.sh), both installed by mise) shows the documents in `docs/`, `CONTRIBUTING`
and `SECURITY`: edit those files, not copies. `mise run site:dev` serves it locally with live
reload; `mise run site:build` builds it into `site/dist`.

Besides Rust, macOS and Windows need a C compiler to build the bundled hidapi library: the
Xcode Command Line Tools on macOS (`xcode-select --install`), and on Windows the MSVC build
tools that Rust requires anyway. On Linux the HID backend talks to `hidraw` directly, so
nothing else is needed.

## Where code belongs

The workspace is split by responsibility. The rule of thumb: **device-specific bytes live in
exactly one driver crate; nothing else knows about them.**

| You want to... | Put it in |
|---|---|
| Support a new display model | `crates/drivers/<model>/` (new crate) |
| Change how a known display is spoken to | `crates/drivers/<model>/src/protocol.rs` |
| Add something every display needs (a capability, encoding, transport, pacing) | `crates/core/` |
| Add a built-in screen (like the clock) | `crates/server/src/sources/` |
| Add or change an API endpoint | `crates/server/src/api/` (+ `docs/api.md`) |
| Add a config option | `crates/server/src/config.rs` (+ `TEMPLATE` there) |
| Add a CLI command | `crates/cli/src/main.rs` (+ `docs/cli.md` / `docs/cli.ja.md`) |
| Change autostart for an OS | `crates/cli/src/service.rs` |
| Linux permissions for a device | `contrib/linux/70-sub-screen-player.rules` |
| The website's home page (text in `copy.ts`) | `site/src/components/landing/` |
| A screen for the gallery | `site/public/screens/<name>/` (+ `site/src/components/gallery/screens.ts`) |
| Website-only pages in the docs (getting started, download) | `site/content/` (and `site/content/ja/`) |
| Publish another document on the website | `site/scripts/sync-docs.ts` and the sidebar in `site/astro.config.mjs` |

Dependencies point one way only: `cli` → `server` → `drivers/*` → `core`. A driver never
depends on another driver or on the server. [docs/architecture.md](docs/architecture.md)
explains the layers in detail.

## Coding guidelines

- Follow `rustfmt` and keep `clippy` clean (`mise run ci` must pass).
- `unsafe` code is forbidden in this workspace.
- Code, comments and commit messages are in English.
- Documentation exists in English (`*.md`) and Japanese (`*.ja.md`). When you change one,
  update the other as well. If you cannot write one of the languages, say so in the pull
  request and someone will help with the translation.
- Public items have doc comments. Explain *why* in comments, not *what*.
- Keep protocol code pure: functions that build bytes, with no I/O, so they can be tested
  byte for byte.
- Prefer small, focused crates and modules over large ones. New dependencies need a reason
  in the pull request; avoid ones that pull in C libraries.

## Tests

Everything is testable without hardware:

- `ssp_core::testing::RecordingTransport` records the reports a driver sends.
- `ssp_core::testing::FakeDisplay` stands in for a display in presenter and server tests.

Driver tests should compare the reports, byte for byte, with ones known to work on the
device. If you also tested on real hardware, say what you checked in the pull request
(model, OS, what was shown), and paste the output of `ssp selftest`.

## Commits and pull requests

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(d92): support the boot logo upload
fix(server): keep the clock running after a replug
docs: explain the stream format
```

Common scopes: `core`, `d92` (or another driver id), `server`, `cli`, `docs`, `ci`.

Before opening a pull request:

- [ ] `mise run ci` passes.
- [ ] New behavior has tests.
- [ ] User-visible changes are reflected in the README and `docs/`, in English and Japanese.
- [ ] Hardware testing is described (or stated as not done).

## Etiquette for investigating a device's communication

Many displays have no public documentation, so drivers are often built by trying commands on
the device.

- Work out protocols for interoperability only. Do not copy code, binaries, firmware, images
  or fonts from vendor software into this repository.
- Document how a fact was found (an experiment, a public SDK) in `docs/devices/<model>.md` (and
  `.ja.md`), and mark what is verified and what is a guess.
- Never commit raw logs or dumps that may contain personal data. Keep only the relevant bytes.
- Some commands can leave a device stuck or overwrite its stored images. Write such findings
  down and make sure the driver never sends dangerous commands by accident.

## License

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as in
[README.md](README.md#license), without any additional terms or conditions.
