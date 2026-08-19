#!/usr/bin/env bash

# SPDX-FileCopyrightText: 2026 Phillip Cloud
#
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

if [[ ${1-} != -C || ${2-} != /worktrees/project-alpha ]]; then
	echo "unexpected fake Git arguments" >&2
	exit 2
fi
shift 2

case "$*" in
"symbolic-ref --quiet --short HEAD")
	echo "feature/source-scope"
	;;
remote)
	echo "origin"
	;;
"remote get-url origin")
	echo "ssh://git@Example.Invalid:22/acme%2Ftmp%2F..%2Fproject-alpha.git"
	;;
*)
	echo "unexpected fake Git command" >&2
	exit 2
	;;
esac
