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
	# release list (already newest-first) for the first tag with this
	# tool's "<tool>-v" prefix.
	RELEASES_JSON="$(curl -fsSL "$API_BASE/repos/kys0213/areum-lab-tools/releases?per_page=100")"
	# Anchor on "-v<digit>" so a similarly-prefixed tool's tag (e.g. a
	# "discord-vx-v9.9.9" release for tool "discord-vx") is never mistaken
	# for this tool's release via plain substring matching.
	TAG="$(printf '%s\n' "$RELEASES_JSON" | grep -o "\"tag_name\": *\"${TOOL}-v[0-9][^\"]*\"" | head -n1 | sed -E "s/\"tag_name\": *\"${TOOL}-v(.*)\"/\1/")"
	if [ -z "$TAG" ]; then
		echo "error: no release found for $TOOL" >&2
		exit 1
	fi
	VERSION="$TAG"
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
