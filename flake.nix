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
        aarch64-darwin = {
          platform = "mac-arm64";
          sha256 = "52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40";
        };
        # No x86_64-darwin: nixpkgs-unstable dropped Intel Macs.
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

          shellHook = ''
            export CARGO_HOME="''${CARGO_HOME:-$HOME/.cargo}"
          '';

          # Cross targets for `make all` on a Mac: x86_64 Linux and both Windows architectures.
          crossTargets = [
            "x86_64-unknown-linux-gnu"
            "x86_64-pc-windows-msvc"
            "aarch64-pc-windows-msvc"
          ];

          # The x86_64 Linux libraries the app links by name (xcb, xkbcommon, fontconfig) and the
          # ones their pkg-config files require. Fetched prebuilt from the Nix cache; zig supplies
          # glibc. At run time the binary loads the distribution's own copies.
          linuxPkgs = import nixpkgs { system = "x86_64-linux"; };
          linuxSysroot = pkgs.symlinkJoin {
            name = "quire-linux-x64-libs";
            paths = lib.concatMap (p: [
              (lib.getLib p)
              (lib.getDev p)
            ]) (
              with linuxPkgs;
              [
                libxcb
                libxau
                libxdmcp
                libxkbcommon
                fontconfig
                freetype
                expat
                zlib
                bzip2
                libpng
                brotli
              ]
            );
          };

          # Only the LLVM tools cargo-xwin calls for the Windows targets. Putting all of LLVM's
          # bin on PATH would shadow Xcode's clang for the Mac build.
          llvm = pkgs.llvmPackages;
          # Wrappers for two C dependencies that do not cross-compile from a Mac as is; each
          # script says why.
          wrapper =
            name:
            pkgs.writeShellScript name (
              lib.replaceStrings [ "@llvm@" ] [ "${llvm.clang-unwrapped}" ] (builtins.readFile ./nix/${name}.sh)
            );
          windowsTools = pkgs.runCommand "quire-windows-cross-tools" { } ''
            mkdir -p $out/bin
            ln -s ${llvm.lld}/bin/lld-link ${llvm.llvm}/bin/llvm-lib ${llvm.llvm}/bin/llvm-rc \
              ${llvm.llvm}/bin/llvm-ar $out/bin/
            ln -s ${wrapper "clang"} $out/bin/clang
            ln -s ${wrapper "clang-cl"} $out/bin/clang-cl
          '';

          # macOS keeps Xcode's clang, linker and SDK: GPUI compiles its Metal shaders with
          # `xcrun metal`, which Nix's SDK does not ship. The Rust package propagates Nix's
          # clang wrapper and its flags, so the hook drops them and the build matches a plain
          # Xcode build.
          darwinShell = pkgs.mkShellNoCC {
            packages = with pkgs; [
              (rust.override { targets = crossTargets; })
              bun
              nodejs_26
              cmake
              pkg-config
              cargo-zigbuild
              zig
              cargo-xwin
              windowsTools
            ];
            env = {
              QUIRE_PDFIUM_DIR = "${pdfium}";
              PKG_CONFIG_PATH_x86_64_unknown_linux_gnu = "${linuxSysroot}/lib/pkgconfig";
              PKG_CONFIG_ALLOW_CROSS = "1";
              # The Nix libraries above need a newer glibc than the binary targets; the binary
              # loads the distribution's own copies, so their glibc needs are not checked here.
              CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS = "-L native=${linuxSysroot}/lib -C link-arg=-Wl,--allow-shlib-undefined";
            };
            shellHook = shellHook + ''
              for var in $(compgen -e | grep -E '^(NIX_(CC|BINTOOLS|CFLAGS|LDFLAGS|HARDENING|ENFORCE|DONT_SET|NO_SELF|IGNORE_LD)|(CC|CXX|LD|AR)(_FOR_BUILD)?$|MACOSX_DEPLOYMENT_TARGET$)'); do
                unset "$var"
              done
              PATH=$(printf %s "$PATH" | tr : '\n' | grep -vE '/nix/store/[^/]*-(clang-wrapper|clang|cctools-binutils-darwin(-wrapper)?)-[^/]*/bin' | paste -sd : -)
              export PATH
              if ! /usr/bin/xcrun --find metal >/dev/null 2>&1; then
                echo "quire: Xcode's Metal compiler was not found. Install Xcode, then run: sudo xcode-select -s /Applications/Xcode.app" >&2
              fi
            '';
          };
        in
        if pkgs.stdenv.hostPlatform.isDarwin then
          darwinShell
        else
        pkgs.mkShell {
          packages = with pkgs; [
            rust
            bun
            nodejs_26
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

          inherit shellHook;
        };
    in
    {
      devShells = nixpkgs.lib.genAttrs (builtins.attrNames pdfiumArchives) (system: {
        default = forSystem system;
      });
    };
}
