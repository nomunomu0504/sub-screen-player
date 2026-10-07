# Security policy

[日本語](SECURITY.ja.md)

## Supported versions

The project is at an early stage. Security fixes go to the latest commit on `main` (and, once
releases exist, the latest release).

## Reporting a vulnerability

**Please do not open a public issue.** Report it privately instead:

1. Open the repository's [Security tab](https://github.com/nomunomu0504/sub-screen-player/security)
   and choose **Report a vulnerability**.
2. Include the affected version or commit, your OS, steps to reproduce (or a proof of concept)
   and what an attacker could do with it.

This is a volunteer project. We aim to acknowledge a report within a week and will keep you
informed until it is fixed. Reporters are credited in the advisory unless they prefer not to
be.

## Scope

In scope:

- The daemon's API: bypassing the token or the `Host`/`Origin` checks, reaching the API from
  a web page, or the API becoming reachable from the network without a token.
- Crashes, hangs or resource exhaustion caused by API input (images, frames, JSON).
- Autostart entries (`ssp service install`) that can be abused, e.g. through unsafe paths or
  permissions.
- Input that makes the daemon send commands that damage or lock up a display, such as
  commands that wedge the device or overwrite its boot logo.

Out of scope:

- Attacks that need physical access to the computer or the display.
- Configurations that deliberately expose the API with a weak or published token.
- Flaws in display firmware itself. Please report those to the vendor; we are happy to
  document workarounds.

## Security model

By default the API listens on localhost only, refuses requests from web pages, and requires
a token for any other address. See
[docs/architecture.md](docs/architecture.md#security-model) for details.
