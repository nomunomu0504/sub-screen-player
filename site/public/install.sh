#!/bin/sh
# Installs ssp (sub-screen-player) from the latest GitHub release on macOS or Linux.
#
#   curl -fsSL https://subscreen.dev/install.sh | sh
#
# Environment:
#   SSP_VERSION      release to install, e.g. v0.1.0 (default: the latest)
#   SSP_INSTALL_DIR  where to put ssp (default: ~/.local/bin)
set -eu

repo="nomunomu0504/sub-screen-player"
version="${SSP_VERSION:-}"
dir="${SSP_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() {
	printf 'error: %s\n' "$*" >&2
	exit 1
}
for tool in curl tar uname mktemp; do
	command -v "$tool" > /dev/null 2>&1 || fail "$tool is required"
done

case "$(uname -s)" in
	Darwin) target=universal-apple-darwin ;;
	Linux)
		case "$(uname -m)" in
			x86_64 | amd64) target=x86_64-unknown-linux-musl ;;
			aarch64 | arm64) target=aarch64-unknown-linux-musl ;;
			*) fail "unsupported CPU: $(uname -m)" ;;
		esac
		;;
	*) fail "unsupported system: $(uname -s) (on Windows, use install.ps1)" ;;
esac

if [ -z "$version" ]; then
	# The latest release page redirects to .../releases/tag/<version>.
	latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest") ||
		fail "cannot reach GitHub"
	version=${latest##*/}
fi
case "$version" in
	v*) ;;
	*) fail "cannot tell the latest version (got '$version')" ;;
esac

name="ssp-$version-$target"
url="https://github.com/$repo/releases/download/$version"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "Downloading ssp $version for $target"
curl -fsSL -o "$tmp/$name.tar.gz" "$url/$name.tar.gz" || fail "download failed: $url/$name.tar.gz"
curl -fsSL -o "$tmp/SHA256SUMS.txt" "$url/SHA256SUMS.txt" || fail "download failed: SHA256SUMS.txt"

expected=$(grep " $name.tar.gz\$" "$tmp/SHA256SUMS.txt" | cut -d ' ' -f 1)
[ -n "$expected" ] || fail "no checksum for $name.tar.gz"
if command -v sha256sum > /dev/null 2>&1; then
	actual=$(sha256sum "$tmp/$name.tar.gz" | cut -d ' ' -f 1)
else
	actual=$(shasum -a 256 "$tmp/$name.tar.gz" | cut -d ' ' -f 1)
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $name.tar.gz"

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$dir"
cp "$tmp/$name/ssp" "$dir/ssp.new"
chmod 755 "$dir/ssp.new"
mv -f "$dir/ssp.new" "$dir/ssp"
say "Installed $("$dir/ssp" --version) to $dir/ssp"

case ":$PATH:" in
	*":$dir:"*) ;;
	*) say "Note: $dir is not on your PATH. Add it, e.g.: echo 'export PATH=\"$dir:\$PATH\"' >> ~/.profile" ;;
esac

if [ "$target" != universal-apple-darwin ] && [ ! -e /etc/udev/rules.d/70-sub-screen-player.rules ]; then
	rules="${XDG_DATA_HOME:-$HOME/.local/share}/sub-screen-player/70-sub-screen-player.rules"
	mkdir -p "$(dirname "$rules")"
	cp "$tmp/$name/70-sub-screen-player.rules" "$rules"
	say ""
	say "To use displays without root, install the udev rule and replug the display:"
	say "  sudo cp '$rules' /etc/udev/rules.d/ && sudo udevadm control --reload-rules"
fi

say ""
say "Next: plug in the display and run 'ssp serve'."
