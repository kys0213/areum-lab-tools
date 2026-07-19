#!/bin/sh
# Install a tool from GitHub Release into $INSTALL_ROOT/bin.
#
# Usage: install.sh <tool> [version]
#
# Supports `curl -fsSL .../install.sh | sh -s -- <tool> [version]`, so this
# script must never read stdin.
set -eu

usage() {
	echo "usage: install.sh <tool> [version]" >&2
}

TOOL="${1:-}"
VERSION="${2:-}"

if [ -z "$TOOL" ]; then
	usage
	exit 1
fi

# Only aarch64-apple-darwin release artifacts exist today; fail fast rather
# than downloading a binary that won't run.
OS="$(uname -s)"
ARCH="$(uname -m)"
if [ "$OS" != "Darwin" ] || [ "$ARCH" != "arm64" ]; then
	echo "error: unsupported platform ($OS/$ARCH) - only Darwin/arm64 (aarch64-apple-darwin) is supported" >&2
	exit 1
fi
TARGET="aarch64-apple-darwin"

API_BASE="${AREUM_API_BASE:-https://api.github.com}"
DOWNLOAD_BASE="${AREUM_DOWNLOAD_BASE:-https://github.com/kys0213/areum-lab-tools/releases/download}"
INSTALL_ROOT="${INSTALL_ROOT:-$HOME/.local}"

if [ -z "$VERSION" ]; then
	# GitHub's /releases/latest is repo-wide, not per-tool, so we scan the
	# release list (already newest-first) and pick the first tag belonging
	# to this tool.
	RELEASES_JSON="$(curl -fsSL "$API_BASE/repos/kys0213/areum-lab-tools/releases?per_page=100")"
	# grep only extracts candidate tag names; ownership is decided by
	# splitting each tag at its last "-v" (same convention as release.yml)
	# and requiring the tool part to equal $TOOL exactly. Prefix or
	# substring heuristics mismatch similarly named tools (e.g. tags
	# "discord-vx-v9.9.9" or "discord-v2-v1.0.0" when installing "discord").
	TAG_CANDIDATES="$(printf '%s\n' "$RELEASES_JSON" | grep -o '"tag_name": *"[^"]*-v[^"]*"' | sed -E 's/"tag_name": *"([^"]*)"/\1/')"
	for tag in $TAG_CANDIDATES; do
		if [ "${tag%-v*}" = "$TOOL" ]; then
			VERSION="${tag##*-v}"
			break
		fi
	done
	if [ -z "$VERSION" ]; then
		echo "error: no release found for $TOOL" >&2
		exit 1
	fi
fi

ASSET="$TOOL-$VERSION-$TARGET.tar.gz"
RELEASE_TAG="$TOOL-v$VERSION"
DOWNLOAD_URL="$DOWNLOAD_BASE/$RELEASE_TAG/$ASSET"

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

curl -fsSL -o "$WORK_DIR/$ASSET" "$DOWNLOAD_URL"
curl -fsSL -o "$WORK_DIR/$ASSET.sha256" "$DOWNLOAD_URL.sha256"

# Fail fast on any checksum mismatch rather than installing an unverified
# binary. Run from within WORK_DIR so the .sha256's recorded filename
# (a bare basename) resolves correctly.
(cd "$WORK_DIR" && shasum -a 256 -c "$ASSET.sha256")

BIN_DIR="$INSTALL_ROOT/bin"
mkdir -p "$BIN_DIR"
tar xzf "$WORK_DIR/$ASSET" -C "$WORK_DIR"

if [ ! -f "$WORK_DIR/$TOOL" ]; then
	echo "error: expected binary '$TOOL' not found in $ASSET" >&2
	exit 1
fi

chmod +x "$WORK_DIR/$TOOL"
mv "$WORK_DIR/$TOOL" "$BIN_DIR/$TOOL"

echo "installed $TOOL $VERSION to $BIN_DIR/$TOOL"
