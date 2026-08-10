{
  coreutils,
  lib,
  writeShellApplication,
}:
{
  fakeApiBin,
  tuiBin,
}:
writeShellApplication {
  name = "herdr-agentsview-demo";
  text =
    builtins.replaceStrings
      [
        "@@FAKE_API@@"
        "@@ENV@@"
        "@@MKDIR@@"
        "@@MKTEMP@@"
        "@@REALPATH@@"
        "@@RM@@"
        "@@SLEEP@@"
        "@@TUI@@"
      ]
      [
        fakeApiBin
        (lib.getExe' coreutils "env")
        (lib.getExe' coreutils "mkdir")
        (lib.getExe' coreutils "mktemp")
        (lib.getExe' coreutils "realpath")
        (lib.getExe' coreutils "rm")
        (lib.getExe' coreutils "sleep")
        tuiBin
      ]
      (builtins.readFile ./demo.sh);
}
