#!/bin/sh
# Smoke tests for scripts/install.sh.
#
# Spins up a local `python3 -m http.server` per case, serving a fake
# GitHub-releases-shaped fixture tree, and runs install.sh against it via the
# AREUM_API_BASE / AREUM_DOWNLOAD_BASE / INSTALL_ROOT overrides. No network
# access and no real GitHub release is required.
#
# Usage: scripts/install_test.sh
set -u

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_SH="$SCRIPT_DIR/install.sh"

WORK_DIR="$(mktemp -d)"
SERVER_PID=""
BASE_URL=""
FAIL=0

cleanup() {
	stop_server
	rm -rf "$WORK_DIR"
}
trap cleanup EXIT INT TERM

pass() { echo "PASS: $1"; }
fail() {
	echo "FAIL: $1"
	FAIL=1
}

find_free_port() {
	python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

# start_server <dir> - serve <dir> over HTTP on 127.0.0.1, set $BASE_URL.
start_server() {
	dir="$1"
	port="$(find_free_port)"
	( cd "$dir" && exec python3 -m http.server "$port" --bind 127.0.0.1 >"$WORK_DIR/server.log" 2>&1 ) &
	SERVER_PID=$!
	BASE_URL="http://127.0.0.1:$port"

	tries=0
	while ! curl -fsS -o /dev/null "$BASE_URL/" 2>/dev/null; do
		tries=$((tries + 1))
		if [ "$tries" -ge 50 ]; then
			echo "error: http.server did not become ready on $BASE_URL" >&2
			return 1
		fi
		sleep 0.1
	done
}

stop_server() {
	if [ -n "$SERVER_PID" ]; then
		kill "$SERVER_PID" 2>/dev/null || true
		wait "$SERVER_PID" 2>/dev/null || true
		SERVER_PID=""
	fi
}

# make_asset <tarball_path> <binary_name> <echo_content>
#
# Builds a tarball containing an executable shell script named <binary_name>
# that prints <echo_content>, plus a <tarball_path>.sha256 in `shasum -a 256`
# check format (recorded filename is a bare basename, matching how
# install.sh runs `shasum -a 256 -c` from within its own work dir).
make_asset() {
	tarball="$1"
	binname="$2"
	content="$3"
	tmpbin="$(mktemp -d)"
	printf '#!/bin/sh\necho "%s"\n' "$content" >"$tmpbin/$binname"
	chmod +x "$tmpbin/$binname"
	outdir="$(dirname "$tarball")"
	mkdir -p "$outdir"
	tar czf "$tarball" -C "$tmpbin" "$binname"
	( cd "$outdir" && shasum -a 256 "$(basename "$tarball")" >"$(basename "$tarball").sha256" )
	rm -rf "$tmpbin"
}

# releases_fixture <www_dir> <json_body> - write the fake GitHub releases
# API response at the path install.sh actually requests (query string is
# stripped by http.server's path translation, so a plain file works).
releases_fixture() {
	www="$1"
	json="$2"
	mkdir -p "$www/api/repos/kys0213/areum-lab-tools"
	printf '%s' "$json" >"$www/api/repos/kys0213/areum-lab-tools/releases"
}

# run_install <www_dir> <install_root> <tool> [version]
# Starts a server for <www_dir>, runs install.sh, stops the server, and
# leaves stdout/stderr in $STDOUT_LOG/$STDERR_LOG plus the exit code in $RC.
run_install() {
	www="$1"
	install_root="$2"
	tool="$3"
	version="${4:-}"
	STDOUT_LOG="$WORK_DIR/last-stdout.log"
	STDERR_LOG="$WORK_DIR/last-stderr.log"

	start_server "$www" || {
		RC=1
		: >"$STDOUT_LOG"
		echo "error: could not start fixture server" >"$STDERR_LOG"
		return
	}

	AREUM_API_BASE="$BASE_URL/api" AREUM_DOWNLOAD_BASE="$BASE_URL/download" INSTALL_ROOT="$install_root" \
		"$INSTALL_SH" "$tool" "$version" >"$STDOUT_LOG" 2>"$STDERR_LOG"
	RC=$?
	stop_server
}

# --- (a) normal install: version unspecified, picks the newest release ---
case_normal_install() {
	label="(a) normal install (version unspecified)"
	case_dir="$WORK_DIR/case_a"
	www="$case_dir/www"

	releases_fixture "$www" '[
  {"tag_name": "discord-v1.2.3"},
  {"tag_name": "hello-v0.1.0"}
]'
	make_asset "$www/download/discord-v1.2.3/discord-1.2.3-aarch64-apple-darwin.tar.gz" "discord" "discord-v1.2.3-ok"

	install_root="$case_dir/install_root"
	run_install "$www" "$install_root" "discord"

	if [ "$RC" -ne 0 ]; then
		fail "$label: install.sh exited $RC (expected 0); stderr: $(cat "$STDERR_LOG")"
		return
	fi
	if [ ! -x "$install_root/bin/discord" ]; then
		fail "$label: binary not installed at $install_root/bin/discord"
		return
	fi
	output="$("$install_root/bin/discord")"
	if [ "$output" != "discord-v1.2.3-ok" ]; then
		fail "$label: unexpected binary output: $output"
		return
	fi
	pass "$label"
}

# --- (b) checksum mismatch aborts the install and leaves nothing behind ---
case_checksum_mismatch() {
	label="(b) checksum mismatch aborts install"
	case_dir="$WORK_DIR/case_b"
	www="$case_dir/www"

	releases_fixture "$www" '[
  {"tag_name": "discord-v1.0.0"}
]'
	asset="$www/download/discord-v1.0.0/discord-1.0.0-aarch64-apple-darwin.tar.gz"
	make_asset "$asset" "discord" "discord-v1.0.0-ok"
	# Corrupt the tarball after its checksum was recorded so verification fails.
	printf 'corruption' >>"$asset"

	install_root="$case_dir/install_root"
	run_install "$www" "$install_root" "discord"

	if [ "$RC" -eq 0 ]; then
		fail "$label: install.sh exited 0, expected non-zero on checksum mismatch"
		return
	fi
	if [ -e "$install_root/bin/discord" ]; then
		fail "$label: binary was installed despite checksum mismatch"
		return
	fi
	pass "$label"
}

# --- (c) no matching release errors clearly instead of silently failing ---
case_no_release() {
	label="(c) no release found errors clearly"
	case_dir="$WORK_DIR/case_c"
	www="$case_dir/www"

	releases_fixture "$www" '[
  {"tag_name": "hello-v0.1.0"}
]'

	install_root="$case_dir/install_root"
	run_install "$www" "$install_root" "discord"

	if [ "$RC" -eq 0 ]; then
		fail "$label: install.sh exited 0, expected non-zero when no release exists"
		return
	fi
	if ! grep -q "no release found for discord" "$STDERR_LOG"; then
		fail "$label: expected error message not found in stderr: $(cat "$STDERR_LOG")"
		return
	fi
	pass "$label"
}

# --- (d) a similarly-named tool's tag must not be mismatched onto ours ---
case_similar_name_guard() {
	label="(d) similar-named tool tag is not mismatched"
	case_dir="$WORK_DIR/case_d"
	www="$case_dir/www"

	# "discord-vx" is a distinct tool whose tag happens to contain "discord-v"
	# as a substring. It is listed first (newest) specifically to catch a
	# substring-matching tag filter that would pick it for TOOL=discord.
	releases_fixture "$www" '[
  {"tag_name": "discord-vx-v9.9.9"},
  {"tag_name": "discord-v1.0.0"}
]'
	# What a substring-matching filter would wrongly resolve to and download.
	make_asset "$www/download/discord-vx-v9.9.9/discord-x-v9.9.9-aarch64-apple-darwin.tar.gz" "discord" "WRONG-discord-vx-binary"
	# The actual "discord" release.
	make_asset "$www/download/discord-v1.0.0/discord-1.0.0-aarch64-apple-darwin.tar.gz" "discord" "discord-v1.0.0-ok"

	install_root="$case_dir/install_root"
	run_install "$www" "$install_root" "discord"

	if [ "$RC" -ne 0 ]; then
		fail "$label: install.sh exited $RC (expected 0; 'discord' has a real v1.0.0 release); stderr: $(cat "$STDERR_LOG")"
		return
	fi
	output="$("$install_root/bin/discord" 2>/dev/null || true)"
	if [ "$output" != "discord-v1.0.0-ok" ]; then
		fail "$label: installed the wrong release (got '$output'); tag filter matched discord-vx-v9.9.9 by substring instead of anchoring to discord-v<digit>"
		return
	fi
	pass "$label"
}

case_normal_install
case_checksum_mismatch
case_no_release
case_similar_name_guard

if [ "$FAIL" -ne 0 ]; then
	echo "install.sh smoke tests: FAILED"
	exit 1
fi
echo "install.sh smoke tests: all passed"
