# Dev shell for macrotool-gtk4 — mirrors pkgs/macrotool-gtk4/default.nix
# buildInputs so we can run `cargo check` / `cargo test` outside the nix
# sandbox on a NixOS host. Used by Jaide to iterate before bumping rev+cargoHash
# in nix-config and rebuilding the system profile.
#
# Usage:  nix-shell --run 'cargo test'
#
# This file is dev-only and is NOT referenced by the NixOS build. The
# system profile build keeps using buildRustPackage in default.nix.
{ pkgs ? import <nixpkgs> {} }:
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    cargo
    rustc
    pkg-config
    wrapGAppsHook4
  ];
  buildInputs = with pkgs; [
    gtk4
    gtk4-layer-shell
    glib.dev
    libx11.dev
    pango
    cairo
    gdk-pixbuf
    libepoxy
    graphene
    gobject-introspection
    harfbuzz
    libsoup_3
    wayland
    libxkbcommon
    xorg.libXcursor
    xorg.libXrandr
    xorg.libXi
    xorg.libXext
    xorg.libXdamage
    xorg.libXfixes
    xorg.libXrender
    xorg.libXinerama
    xorg.libXcomposite
  ];
  shellHook = ''
    # gtk4-layer-shell.pc lives in the .dev output but nix-shell only adds
    # the runtime lib by default; expose the .dev pkgconfig directly so the
    # buildRustPackage-style build can find it.
    for d in ${pkgs.gtk4-layer-shell.dev}/lib/pkgconfig \
             ${pkgs.libxkbcommon.dev}/lib/pkgconfig \
             ${pkgs.wayland.dev}/lib/pkgconfig; do
      export PKG_CONFIG_PATH="$d:$PKG_CONFIG_PATH"
    done
    export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:${pkgs.gtk4}/lib:${pkgs.gtk4-layer-shell}/lib"
  '';
}