# Applications: terminal, launcher, control panels
{ pkgs }:

with pkgs; [
  # Terminal
  kitty
  tmux

  # Launcher

  # Control panels
  networkmanagerapplet
  blueman
  pavucontrol
  easyeffects
  gradia

  # Icons
  kdePackages.breeze-icons
  hicolor-icon-theme
]
