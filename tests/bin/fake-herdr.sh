#!/usr/bin/env bash

# SPDX-FileCopyrightText: 2026 Phillip Cloud
#
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

: "${FAKE_HERDR_DIR:?}"

{
	first=true
	for arg in "$@"; do
		if [[ $first == true ]]; then
			first=false
		else
			printf '\t'
		fi
		printf '%s' "$arg"
	done
	printf '\n'
} >>"$FAKE_HERDR_DIR/calls"

case "${FAKE_HERDR_MODE-success}" in
failure)
	echo "fake Herdr failure" >&2
	exit 17
	;;
hang)
	echo "$$" >"$FAKE_HERDR_DIR/pid"
	exec @@SLEEP@@ 10
	;;
success)
	;;
*)
	echo "unknown fake Herdr mode" >&2
	exit 2
	;;
esac
