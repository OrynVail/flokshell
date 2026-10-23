# Main Flokshell package
{ pkgs, lib, self, system, axctl, version }:

let
  quickshellPkg = pkgs.quickshell;
  axctlPkg = axctl.packages.${system}.default;

  # Import sub-packages
  ttf-phosphor-icons = import ./phosphor-icons.nix { inherit pkgs; };
  flokd = import ./flokd.nix { inherit pkgs lib version; };

  # Import modular package lists
  corePkgs = import ./core.nix { inherit pkgs quickshellPkg; };
  toolsPkgs = import ./tools.nix { inherit pkgs; };
  mediaPkgs = import ./media.nix { inherit pkgs; };
  appsPkgs = import ./apps.nix { inherit pkgs; };
  fontsPkgs = import ./fonts.nix { inherit pkgs ttf-phosphor-icons; };
  tesseractPkgs = import ./tesseract.nix { inherit pkgs; };

  # Combine all packages (NixOS-specific deps handled by the module)
  baseEnv = corePkgs
    ++ [ axctlPkg flokd ]
    ++ toolsPkgs
    ++ mediaPkgs
    ++ appsPkgs
    ++ fontsPkgs
    ++ tesseractPkgs;

  envFlokshell = pkgs.buildEnv {
    name = "Flokshell-env";
    paths = baseEnv;
  };

  # Create fontconfig configuration to find bundled fonts
  fontconfigConf = pkgs.writeTextDir "etc/fonts/conf.d/99-flokshell-fonts.conf" ''
    <?xml version="1.0"?>
    <!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
    <fontconfig>
      <dir>${envFlokshell}/share/fonts</dir>
    </fontconfig>
  '';

  # Copy shell sources to the Nix store
  shellSrc = pkgs.stdenv.mkDerivation {
    pname = "flokshell";
    inherit version;
    src = lib.cleanSource self;
    dontBuild = true;
    installPhase = ''
      mkdir -p $out
      cp -r . $out/
    '';
  };

  launcher = pkgs.writeShellScriptBin "flok" ''
    export FLOKSHELL_QS="${quickshellPkg}/bin/qs"
    export PATH="${envFlokshell}/bin:$PATH"

    # Set QML2_IMPORT_PATH to include modules from envFlokshell (like syntax-highlighting)
    export QML2_IMPORT_PATH="${envFlokshell}/lib/qt-6/qml:$QML2_IMPORT_PATH"
    export QML_IMPORT_PATH="$QML2_IMPORT_PATH"

    # Make bundled fonts available to fontconfig
    export FONTCONFIG_PATH="${fontconfigConf}/etc/fonts:''${FONTCONFIG_PATH:-}"

    # Delegate execution to CLI (now in the Nix store)
    exec ${shellSrc}/cli.sh "$@"
  '';

  # Compat shim: the old command name, kept so existing keybinds keep working.
  compat = pkgs.runCommand "flokshell-compat" { } ''
    mkdir -p $out/bin
    ln -s ${launcher}/bin/flok $out/bin/ambxst
  '';

in pkgs.buildEnv {
  name = "Flokshell-${version}";
  paths = [ envFlokshell launcher compat ];
  meta.mainProgram = "flok";
}
