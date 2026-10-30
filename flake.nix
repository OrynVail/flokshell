{
  description = "flokshell - a Wayland shell for Hyprland, forked from Ambxst";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs, ... }:
    let
      flokshellLib = import ./nix/lib.nix { inherit nixpkgs; };
      version = nixpkgs.lib.removeSuffix "\n" (builtins.readFile ./version);
    in {
      nixosModules.default = { pkgs, lib, ... }: {
        imports = [ ./nix/modules ];
        programs.flokshell.enable = lib.mkDefault true;
        programs.flokshell.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      };

      packages = flokshellLib.forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            config.allowUnfree = true;
          };

          lib = nixpkgs.lib;

          Flokshell = import ./nix/packages {
            inherit pkgs lib self system version;
          };
        in {
          default = Flokshell;
          Flokshell = Flokshell;
        }
      );

      devShells = flokshellLib.forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          Flokshell = self.packages.${system}.default;
        in {
          default = pkgs.mkShell {
            packages = [
              Flokshell

              # flokd
              pkgs.cargo
              pkgs.rustc
              pkgs.clippy
              pkgs.rustfmt
              pkgs.rust-analyzer
              pkgs.pkg-config
            ];
            shellHook = ''
              export QML2_IMPORT_PATH="${Flokshell}/lib/qt-6/qml:$QML2_IMPORT_PATH"
              export QML_IMPORT_PATH="$QML2_IMPORT_PATH"
              echo "Flokshell dev environment loaded."
            '';
          };
        }
      );

      apps = flokshellLib.forAllSystems (system:
        let
          Flokshell = self.packages.${system}.default;
        in {
          default = {
            type = "app";
            program = "${Flokshell}/bin/flok";
          };
        }
      );
    };
}
