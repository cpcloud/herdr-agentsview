#!/usr/bin/env bash
set -euo pipefail

umask 077

demo_parent=$(realpath -- "${TMPDIR:-/tmp}")
demo_root=$(mktemp -d "$demo_parent/herdr-agentsview-demo.XXXXXX")
fake_api_pid=

case "$demo_root" in
"$demo_parent"/herdr-agentsview-demo.*) ;;
*)
	echo "mktemp returned an unexpected demo path" >&2
	exit 1
	;;
esac

cleanup() {
	cleanup_status=$?
	cleanup_failed=0
	trap - EXIT HUP INT TERM

	if [[ -n $fake_api_pid ]]; then
		if kill -0 "$fake_api_pid" 2>/dev/null; then
			kill "$fake_api_pid" 2>/dev/null || cleanup_failed=1
		fi
		wait "$fake_api_pid" 2>/dev/null || true
	fi

	if [[ -d $demo_root ]]; then
		case "$demo_root" in
		"$demo_parent"/herdr-agentsview-demo.*)
			rm -r -- "$demo_root" || cleanup_failed=1
			;;
		*)
			echo "refusing to remove an unexpected demo path" >&2
			cleanup_failed=1
			;;
		esac
	fi

	if ((cleanup_status == 0 && cleanup_failed != 0)); then
		cleanup_status=1
	fi
	exit "$cleanup_status"
}

trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

export HOME="$demo_root/home"
export XDG_CACHE_HOME="$demo_root/cache"
export XDG_CONFIG_HOME="$demo_root/config"
export XDG_DATA_HOME="$demo_root/data"
export XDG_STATE_HOME="$demo_root/state"

config_dir="$demo_root/plugin"
mkdir -p \
	"$HOME" \
	"$XDG_CACHE_HOME" \
	"$XDG_CONFIG_HOME" \
	"$XDG_DATA_HOME" \
	"$XDG_STATE_HOME" \
	"$config_dir"

clean_environment=(
	"HOME=$HOME"
	"NO_PROXY=127.0.0.1,::1"
	"TERM=${TERM:-xterm-256color}"
	"TMPDIR=$demo_parent"
	"XDG_CACHE_HOME=$XDG_CACHE_HOME"
	"XDG_CONFIG_HOME=$XDG_CONFIG_HOME"
	"XDG_DATA_HOME=$XDG_DATA_HOME"
	"XDG_STATE_HOME=$XDG_STATE_HOME"
	"no_proxy=127.0.0.1,::1"
)
if [[ -v COLORTERM ]]; then
	clean_environment+=("COLORTERM=$COLORTERM")
fi
if [[ -v NO_COLOR ]]; then
	clean_environment+=("NO_COLOR=$NO_COLOR")
fi

env -i "${clean_environment[@]}" \
	@@FAKE_API@@ --config-out "$config_dir/config.toml" \
	>"$demo_root/fake-api.log" 2>&1 &
fake_api_pid=$!

for _ in {1..200}; do
	if [[ -s "$config_dir/config.toml" ]]; then
		break
	fi
	if ! kill -0 "$fake_api_pid" 2>/dev/null; then
		echo "synthetic API stopped before the demo was ready" >&2
		exit 1
	fi
	sleep 0.025
done

if [[ ! -s "$config_dir/config.toml" ]]; then
	echo "synthetic API did not become ready within 5 seconds" >&2
	exit 1
fi

env -i "${clean_environment[@]}" \
	"HERDR_PLUGIN_CONFIG_DIR=$config_dir" \
	@@TUI@@ tui
