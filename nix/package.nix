{
  bash,
  coreutils,
  herdr,
  lib,
  makeWrapper,
  rustPlatform,
  stdenv,
}:
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "herdr-agentsview";
  version = "0.1.0";

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
    substitute ${../herdr-plugin.toml.in} "$plugin_root/herdr-plugin.toml" \
      --replace-fail '@HERDR_AGENTSVIEW@' "$out/bin/herdr-agentsview"
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
    license = lib.licenses.mit;
    mainProgram = "herdr-agentsview";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ];
  };
})
