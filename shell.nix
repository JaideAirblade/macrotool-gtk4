# Dev shell for building macrotool-gtk4 outside the nix-config flake.
# Recreates the build env the nix package uses (buildRustPackage deps),
# so `cargo build --release` has gtk4/pkg-config/etc. available.
# The repo previously had a shell.nix; it was lost somewhere along the way.
{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    pkg-config
    rustc
    cargo
  ];
  buildInputs = with pkgs; [
    gtk4
    gtk4-layer-shell
    glib
    libX11
  ];

  # Ensure the linker can find the libs at runtime-link time.
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
    pkgs.gtk4
    pkgs.gtk4-layer-shell
    pkgs.glib
    pkgs.libX11
  ];
}
