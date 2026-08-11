# SPDX-FileCopyrightText: 2026 Phillip Cloud
#
# SPDX-License-Identifier: Apache-2.0

{ coreutils, writeShellApplication }:
{ fakeApiBin, tuiBin }:
writeShellApplication {
  name = "herdr-agentsview-demo";
  runtimeInputs = [ coreutils ];
  text = builtins.replaceStrings [ "@@FAKE_API@@" "@@TUI@@" ] [ fakeApiBin tuiBin ] (
    builtins.readFile ./demo.sh
  );
}
