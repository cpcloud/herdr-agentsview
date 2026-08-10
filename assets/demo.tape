# Capture the real TUI against the demo's ephemeral synthetic API.
Require nix

Set Shell "bash"
Set FontSize 16
Set Width 1280
Set Height 960
Set TypingSpeed 20ms

Type "nix run .#demo"
Enter
Wait+Screen /Updated [0-9]+s ago/
Wait+Screen /Peak 2/
Sleep 250ms
Screenshot assets/dashboard.png
Sleep 1s
Type "q"
Sleep 500ms
