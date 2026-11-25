#!/bin/sh
# Print the GitHub Actions build matrix for one tool's release.
#
# Usage: release_targets.sh <tool>
#
# Targets come from tools/<tool>/release-targets (one target triple per line,
# `#` comments and blank lines allowed). A tool without the file ships the
# repository default, aarch64-apple-darwin. Each target is paired with the
# runner that builds it; a target with no known runner fails here, before any
# build starts, instead of failing mid-matrix.
#
# Output (stdout): {"include":[{"target":"...","runner":"..."},...]}
# AREUM_TOOLS_DIR overrides the tools/ directory for the tests.
set -eu

TOOL="${1:-}"
if [ -z "$TOOL" ]; then
	echo "usage: release_targets.sh <tool>" >&2
	exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOLS_DIR="${AREUM_TOOLS_DIR:-$ROOT/tools}"
DECLARED="$TOOLS_DIR/$TOOL/release-targets"
DEFAULT_TARGET="aarch64-apple-darwin"

if [ -f "$DECLARED" ]; then
	TARGETS="$(sed -e 's/#.*//' -e 's/[[:space:]]//g' "$DECLARED" | grep -v '^$' || true)"
else
	TARGETS="$DEFAULT_TARGET"
fi

if [ -z "$TARGETS" ]; then
	echo "error: $DECLARED declares no targets" >&2
	exit 1
fi

# Apple Silicon runners build both macOS targets (x86_64 cross-compiles).
runner_for() {
	case "$1" in
	*-apple-darwin) echo "macos-14" ;;
	*) return 1 ;;
	esac
}

ENTRIES=""
for target in $TARGETS; do
	if ! runner="$(runner_for "$target")"; then
		echo "error: no release runner for target $target (tools/$TOOL/release-targets)" >&2
		exit 1
	fi
	entry="{\"target\":\"$target\",\"runner\":\"$runner\"}"
	ENTRIES="${ENTRIES:+$ENTRIES,}$entry"
done

echo "{\"include\":[$ENTRIES]}"
