# SPDX-FileCopyrightText: 2026 Phillip Cloud
#
# SPDX-License-Identifier: Apache-2.0

{
  bash,
  coreutils,
  herdr,
  lib,
  makeWrapper,
  rustPlatform,
  stdenv,
}:
let
  cargoVersion = (lib.importTOML ../Cargo.toml).package.version;
  pluginVersion = (lib.importTOML ../herdr-plugin.toml).version;
in
assert pluginVersion == cargoVersion;
rustPlatform.buildRustPackage (finalAttrs: {
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

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [
    "--bins"
    "--examples"
  ];

  nativeBuildInputs = [ makeWrapper ];

  BASH_BIN_PATH = lib.getExe bash;
  SLEEP_BIN_PATH = lib.getExe' coreutils "sleep";

  postInstall = ''
    install -Dm755 \
      "target/${stdenv.hostPlatform.rust.rustcTarget}/release/examples/fake_agentsview" \
      "$out/libexec/herdr-agentsview/fake_agentsview"

    plugin_root="$out/share/herdr/plugins/local-agentsview"
    mkdir -p "$plugin_root"
    substitute ${../herdr-plugin.toml} "$plugin_root/herdr-plugin.toml" \
      --replace-fail './target/release/herdr-agentsview' "$out/bin/herdr-agentsview"
  '';

  postFixup = ''
    wrapProgram "$out/bin/herdr-agentsview" \
      --set-default HERDR_BIN_PATH ${lib.getExe herdr}
  '';

  passthru = {
    fakeAgentsview = "${finalAttrs.finalPackage}/libexec/herdr-agentsview/fake_agentsview";
    pluginRoot = "${finalAttrs.finalPackage}/share/herdr/plugins/local-agentsview";
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
})
