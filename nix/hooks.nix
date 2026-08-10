{
  git-hooks,
  pkgs,
  src,
}:
git-hooks.lib.${pkgs.stdenv.hostPlatform.system}.run {
  inherit src;
  package = pkgs.prek;

  settings.rust.check.cargoDeps = pkgs.rustPlatform.importCargoLock {
    lockFile = ../Cargo.lock;
  };

  hooks = {
    actionlint.enable = true;
    check-added-large-files.enable = true;
    check-merge-conflicts.enable = true;
    check-symlinks.enable = true;
    deadnix.enable = true;
    end-of-file-fixer.enable = true;
    nixfmt.enable = true;
    ripsecrets.enable = true;
    shellcheck.enable = true;
    shfmt.enable = true;
    statix.enable = true;
    trim-trailing-whitespace.enable = true;
    yamllint.enable = true;
    zizmor.enable = true;

    clippy = {
      enable = true;
      packageOverrides = {
        inherit (pkgs) cargo clippy;
      };
      settings = {
        denyWarnings = true;
        extraArgs = "--all-targets --locked";
        offline = true;
      };
    };

    markdownlint = {
      enable = true;
      settings.configuration.MD013 = false;
    };

    rustfmt = {
      enable = true;
      packageOverrides = {
        inherit (pkgs) cargo rustfmt;
      };
      settings = {
        check = true;
        color = "never";
      };
    };
  };
}
