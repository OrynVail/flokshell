# NixOS module for Flokshell
{ config, lib, pkgs, ... }:

let
  cfg = config.programs.flokshell;
in {
  options.programs.flokshell = {
    enable = lib.mkEnableOption "Flokshell shell";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The Flokshell package to use";
    };

    fonts.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Whether to install Flokshell fonts (including Phosphor Icons)";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    # The shell must never spawn this: a daemon the shell owns cannot outlive a
    # reload, and a failed start it cannot retry is how a stale socket becomes a
    # permanent error loop. See flokd.md §4.
    systemd.user.services.flokd = {
      description = "flokshell service daemon";
      wantedBy = [ "graphical-session.target" ];
      wants = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/flokd";
        Restart = "always";
        RestartSec = 2;
      };
    };

    # Register fonts with fontconfig (NixOS handles this via fonts.packages)
    fonts.packages = lib.mkIf cfg.fonts.enable (with pkgs; [
      roboto
      roboto-mono
      league-gothic
      terminus_font
      terminus_font_ttf
      dejavu_fonts
      liberation_ttf
      nerd-fonts.symbols-only
      noto-fonts
      noto-fonts-color-emoji
      noto-fonts-cjk-sans
      noto-fonts-cjk-serif
      (pkgs.callPackage ../packages/phosphor-icons.nix { })
    ]);

    # Enable recommended services for full functionality
    services.upower.enable = lib.mkDefault true;
    # i2c group and udev rule; DDC brightness needs the user in that group.
    hardware.i2c.enable = lib.mkDefault true;
    services.power-profiles-daemon.enable = lib.mkDefault true;
    programs.gpu-screen-recorder.enable = lib.mkDefault true;
    networking.networkmanager.enable = lib.mkDefault true;
  };
}
