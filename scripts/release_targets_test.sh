#!/bin/sh
# Tests for scripts/release_targets.sh against a throwaway tools directory
# (AREUM_TOOLS_DIR), so the workspace's tools/* glob never sees a fixture.
#
# Usage: scripts/release_targets_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SCRIPT="$SCRIPT_DIR/release_targets.sh"
FIXTURES="$(mktemp -d)"
FAIL=0

cleanup() {
	rm -rf "$FIXTURES"
}
trap cleanup EXIT INT TERM

pass() { echo "PASS: $1"; }
fail() {
	echo "FAIL: $1"
	FAIL=1
}

# fixture_tool <suffix> [release-targets contents] - a tools/ dir with or
# without the declaration file; echoes the tool name.
fixture_tool() {
	name="$1"
	mkdir -p "$FIXTURES/$name"
	if [ "$#" -ge 2 ]; then
		printf '%s' "$2" >"$FIXTURES/$name/release-targets"
	fi
	echo "$name"
}

check_output() {
	label="$1"
	tool="$2"
	expected="$3"
	actual="$(AREUM_TOOLS_DIR="${TOOLS_DIR:-$FIXTURES}" "$SCRIPT" "$tool" 2>&1)"
	if [ "$actual" = "$expected" ]; then
		pass "$label"
	else
		fail "$label: expected '$expected', got '$actual'"
	fi
}

check_output "(a) no declaration ships the default target" \
	"$(fixture_tool default)" \
	'{"include":[{"target":"aarch64-apple-darwin","runner":"macos-14"}]}'

check_output "(b) declared targets keep their order; comments and blanks ignored" \
	"$(fixture_tool both '# macOS
aarch64-apple-darwin

x86_64-apple-darwin  # Intel
')" \
	'{"include":[{"target":"aarch64-apple-darwin","runner":"macos-14"},{"target":"x86_64-apple-darwin","runner":"macos-14"}]}'

tool="$(fixture_tool unknown 'x86_64-unknown-linux-musl
')"
if AREUM_TOOLS_DIR="$FIXTURES" "$SCRIPT" "$tool" >/dev/null 2>&1; then
	fail "(c) unknown target should fail"
elif AREUM_TOOLS_DIR="$FIXTURES" "$SCRIPT" "$tool" 2>&1 | grep -q "no release runner for target x86_64-unknown-linux-musl"; then
	pass "(c) unknown target fails with its name"
else
	fail "(c) unknown target error message missing"
fi

tool="$(fixture_tool empty '# nothing yet
')"
if AREUM_TOOLS_DIR="$FIXTURES" "$SCRIPT" "$tool" >/dev/null 2>&1; then
	fail "(d) an empty declaration should fail"
else
	pass "(d) an empty declaration fails"
fi

TOOLS_DIR="$(dirname "$SCRIPT_DIR")/tools" check_output "(e) doppel ships both macOS targets" "doppel" \
	'{"include":[{"target":"aarch64-apple-darwin","runner":"macos-14"},{"target":"x86_64-apple-darwin","runner":"macos-14"}]}'

if [ "$FAIL" -ne 0 ]; then
	echo "release_targets.sh tests: FAILED"
	exit 1
fi
echo "release_targets.sh tests: all passed"
