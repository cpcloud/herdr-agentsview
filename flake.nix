# SPDX-FileCopyrightText: 2026 Phillip Cloud
#
# SPDX-License-Identifier: Apache-2.0

{
  description = "AgentsView activity, compressed into one very busy terminal.";

  inputs = {
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    herdr = {
      url = "github:ogulcancelik/herdr/master";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    {
      self,
      git-hooks,
      herdr,
      nixpkgs,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      perSystem =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          package = pkgs.callPackage ./nix/package.nix {
            herdr = herdr.packages.${system}.default;
          };
          hooks = import ./nix/hooks.nix {
            inherit git-hooks pkgs;
            src = self;
          };
          mkDemo = pkgs.callPackage ./nix/demo.nix { };
          demo = mkDemo {
            fakeApiBin = package.passthru.fakeAgentsview;
            tuiBin = pkgs.lib.getExe package;
          };
          demoCheck = pkgs.callPackage ./nix/check-demo.nix { inherit mkDemo; };
        in
        {
          inherit
            demo
            demoCheck
            hooks
            package
            pkgs
            ;
        };
    in
    {
      apps = forAllSystems (
        system:
        let
          outputs = perSystem system;
        in
        {
          default = {
            type = "app";
            program = nixpkgs.lib.getExe outputs.package;
          };
          demo = {
            type = "app";
            program = nixpkgs.lib.getExe outputs.demo;
          };
        }
      );

      checks = forAllSystems (
        system:
        let
          outputs = perSystem system;
        in
        {
          demo = outputs.demoCheck;
          inherit (outputs) package;
          pre-commit = outputs.hooks;
        }
      );

      devShells = forAllSystems (
        system:
        let
          outputs = perSystem system;
        in
        {
          default = outputs.pkgs.mkShell {
            inputsFrom = [ outputs.package ];
            packages = outputs.hooks.enabledPackages ++ [
              outputs.pkgs.cargo
              outputs.pkgs.clippy
              outputs.pkgs.prek
              outputs.pkgs.rustc
              outputs.pkgs.rustfmt
              outputs.pkgs.vhs
            ];
            shellHook = outputs.hooks.shellHook;
          };
        }
      );

      packages = forAllSystems (
        system:
        let
          outputs = perSystem system;
        in
        {
          default = outputs.package;
          inherit (outputs) demo;
          herdr-agentsview = outputs.package;
        }
      );
    };
}
