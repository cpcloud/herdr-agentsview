# SPDX-FileCopyrightText: 2026 Phillip Cloud
#
# SPDX-License-Identifier: Apache-2.0

# Capture the real TUI against the demo's ephemeral synthetic API.
Require nix

Set Shell "bash"
Set FontSize 16
Set Width 2060
Set Height 1200
Set TypingSpeed 20ms

Type "nix run .#demo"
Enter
Wait+Screen /Updated [0-9]+s ago/
Wait+Screen /Peak [1-9]/
Sleep 250ms
Screenshot assets/dashboard.png
Sleep 1s
Type "q"
Sleep 500ms
