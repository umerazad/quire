{
  description = "Quire development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      # PDFium build and archive hashes must match tools/pdfium.ts and crates/quire_media/src/pdf/library.rs.
      pdfiumBuild = "chromium/7881";
      pdfiumArchives = {
        x86_64-linux = {
          platform = "linux-x64";
          sha256 = "1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d";
        };
        aarch64-linux = {
          platform = "linux-arm64";
          sha256 = "ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674";
        };
      };

      forSystem =
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          lib = pkgs.lib;

          rust = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
            extensions = [
              "rust-src"
              "rust-analyzer"
            ];
          };

          archive = pdfiumArchives.${system};
          pdfium = pkgs.runCommand "pdfium-${lib.replaceStrings [ "/" ] [ "-" ] pdfiumBuild}" {
            src = pkgs.fetchurl {
              url = "https://github.com/bblanchon/pdfium-binaries/releases/download/${lib.escapeURL pdfiumBuild}/pdfium-${archive.platform}.tgz";
              inherit (archive) sha256;
            };
          } "mkdir -p $out && tar -xzf $src -C $out";

          # Loaded at runtime by GPUI (dlopen) and needed to link.
          runtimeLibs = with pkgs; [
            fontconfig
            freetype
            libxkbcommon
            vulkan-loader
            wayland
            libxcb
            zstd
          ];

          triple = lib.toUpper (lib.replaceStrings [ "-" ] [ "_" ] pkgs.stdenv.hostPlatform.rust.rustcTarget);
        in
        pkgs.mkShell {
          packages = with pkgs; [
            rust
            bun
            nodejs_22
            pkg-config
            cmake
            clang
            mold
          ];
          buildInputs = runtimeLibs;

          env = {
            "CARGO_TARGET_${triple}_LINKER" = "clang";
            "CARGO_TARGET_${triple}_RUSTFLAGS" = "-C link-arg=-fuse-ld=mold";
            ZSTD_SYS_USE_PKG_CONFIG = "1";
            LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
            # Nix's Mesa drivers, so the Nix-linked binary does not load the host's Vulkan drivers.
            VK_DRIVER_FILES = "${pkgs.mesa}/share/vulkan/icd.d";
            QUIRE_PDFIUM_DIR = "${pdfium}";
          };

          shellHook = ''
            export CARGO_HOME="''${CARGO_HOME:-$HOME/.cargo}"
          '';
        };
    in
    {
      devShells = nixpkgs.lib.genAttrs (builtins.attrNames pdfiumArchives) (system: {
        default = forSystem system;
      });
    };
}
