# SPDX-FileCopyrightText: 2026 Phillip Cloud
#
# SPDX-License-Identifier: Apache-2.0

{
  herdr,
  lib,
  makeWrapper,
  naerskLib,
}:
let
  cargoVersion = (lib.importTOML ../Cargo.toml).package.version;
  pluginVersion = (lib.importTOML ../herdr-plugin.toml).version;
  package = naerskLib.buildPackage {
    pname = "herdr-agentsview";
    version = cargoVersion;

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ../Cargo.lock
        ../Cargo.toml
        ../examples
        ../src
        ../tests
      ];
    };

    cargoBuildOptions =
      old:
      old
      ++ [
        "--bins"
        "--examples"
        "--locked"
      ];
    cargoTestOptions =
      old:
      old
      ++ [
        "--all-targets"
        "--locked"
      ];
    doCheck = true;

    overrideMain = old: {
      nativeBuildInputs = old.nativeBuildInputs ++ [ makeWrapper ];

      postInstall = ''
        install -Dm755 \
          "$out/bin/fake_agentsview" \
          "$out/libexec/herdr-agentsview/fake_agentsview"
        rm "$out/bin/fake_agentsview"

        plugin_root="$out/share/herdr/plugins/local-agentsview"
        mkdir -p "$plugin_root"
        substitute ${../herdr-plugin.toml} "$plugin_root/herdr-plugin.toml" \
          --replace-fail './target/release/herdr-agentsview' "$out/bin/herdr-agentsview"
      '';

      postFixup = ''
        wrapProgram "$out/bin/herdr-agentsview" \
          --set-default HERDR_BIN_PATH ${lib.getExe herdr}
      '';
    };

    meta = {
      description = "Terminal-native AgentsView Activity dashboard for Herdr";
      mainProgram = "herdr-agentsview";
      platforms = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
    };
  };
in
assert pluginVersion == cargoVersion;
package
// {
  passthru = {
    fakeAgentsview = "${package}/libexec/herdr-agentsview/fake_agentsview";
    pluginRoot = "${package}/share/herdr/plugins/local-agentsview";
  };
}
