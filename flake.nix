# SPDX-FileCopyrightText: 2026 Phillip Cloud
#
# SPDX-License-Identifier: Apache-2.0

{
  description = "AgentsView activity, compressed into one very busy terminal.";

  inputs = {
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    herdr = {
      url = "github:ogulcancelik/herdr/master";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    naersk = {
      url = "github:nix-community/naersk";
      inputs.fenix.follows = "fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    {
      self,
      fenix,
      git-hooks,
      herdr,
      naersk,
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
          rustToolchain = fenix.packages.${system}.stable.withComponents [
            "cargo"
            "clippy"
            "rustc"
            "rustfmt"
          ];
          naerskLib = naersk.lib.${system}.override {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
          package = pkgs.callPackage ./nix/package.nix {
            herdr = herdr.packages.${system}.default;
            inherit naerskLib;
          };
          hooks = import ./nix/hooks.nix {
            inherit git-hooks pkgs rustToolchain;
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
            rustToolchain
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
              outputs.pkgs.prek
              outputs.rustToolchain
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
