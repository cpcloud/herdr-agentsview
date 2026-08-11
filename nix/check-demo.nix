# SPDX-FileCopyrightText: 2026 Phillip Cloud
#
# SPDX-License-Identifier: Apache-2.0

{
  coreutils,
  lib,
  mkDemo,
  runCommand,
  writeShellApplication,
}:
let
  fakeApi = writeShellApplication {
    name = "fake-demo-api";
    text = ''
      config_out=
      while (( $# > 0 )); do
        case "$1" in
          --config-out)
            config_out="$2"
            shift 2
            ;;
          *)
            echo "unexpected fake API argument: $1" >&2
            exit 2
            ;;
        esac
      done
      if [[ -z "$config_out" ]]; then
        echo "fake API did not receive --config-out" >&2
        exit 2
      fi
      echo "$$" > "$TMPDIR/demo-test-server.pid"
      ${lib.getExe' coreutils "mkdir"} -p "''${config_out%/*}"
      echo 'api_base_url = "http://127.0.0.1:1/"' > "$config_out"
      exec ${lib.getExe' coreutils "sleep"} 3600
    '';
  };

  fakeTui = writeShellApplication {
    name = "fake-demo-tui";
    text = ''
      if [[ $# -ne 1 || $1 != tui ]]; then
        echo "demo did not invoke the standalone TUI command" >&2
        exit 2
      fi
      if [[ -v AGENTSVIEW_SENTINEL || -v HERDR_SENTINEL ]]; then
        echo "demo leaked inherited AgentsView or Herdr environment" >&2
        exit 2
      fi
      case "$HERDR_PLUGIN_CONFIG_DIR" in
        "$TMPDIR"/herdr-agentsview-demo.*/plugin) ;;
        *)
          echo "demo did not use its temporary plugin configuration" >&2
          exit 2
          ;;
      esac
      case "$HOME" in
        "$TMPDIR"/herdr-agentsview-demo.*/home) ;;
        *)
          echo "demo did not isolate HOME" >&2
          exit 2
          ;;
      esac
      if [[ ! -s "$HERDR_PLUGIN_CONFIG_DIR/config.toml" ]]; then
        echo "demo launched the TUI without generated configuration" >&2
        exit 2
      fi
      echo invoked > "$TMPDIR/demo-test-tui.marker"
    '';
  };

  demo = mkDemo {
    fakeApiBin = lib.getExe fakeApi;
    tuiBin = lib.getExe fakeTui;
  };
in
runCommand "herdr-agentsview-demo-check" { nativeBuildInputs = [ demo ]; } ''
  export TMPDIR="$PWD/tmp"
  export AGENTSVIEW_SENTINEL="must be cleared"
  export HERDR_SENTINEL="must be cleared"
  mkdir -p "$TMPDIR"

  herdr-agentsview-demo

  test -s "$TMPDIR/demo-test-tui.marker"
  server_pid=$(< "$TMPDIR/demo-test-server.pid")
  if kill -0 "$server_pid" 2>/dev/null; then
    echo "demo left its fake API process running" >&2
    exit 1
  fi
  for leftover in "$TMPDIR"/herdr-agentsview-demo.*; do
    if [[ -e "$leftover" ]]; then
      echo "demo left temporary files behind" >&2
      exit 1
    fi
  done

  touch "$out"
''
