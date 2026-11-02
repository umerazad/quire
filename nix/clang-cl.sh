#!/usr/bin/env bash
# `clang-cl` for cargo-xwin. A GNU-syntax `.s` file (psm's ARM64 Windows stack switcher) is
# assembled by plain clang with the C preprocessor and the CFG_TARGET_* defines psm's build
# script adds only for non-MSVC compilers; clang-cl cannot preprocess a `.s` file. Every other
# call goes to clang-cl unchanged.
asm=
for arg in "$@"; do
  case "$arg" in *.s) asm=1 ;; esac
done
[ -n "$asm" ] || exec @llvm@/bin/clang-cl "$@"

arch=x86_64
args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --target=aarch64*) arch=aarch64; args+=("$1") ;;
    --target=*) args+=("$1") ;;
    /imsvc) args+=(-isystem "$2"); shift ;;
    -Fo* | /Fo*) args+=(-o "${1:3}") ;;
    -o) args+=(-o "$2"); shift ;;
    -D* | -I* | -c) args+=("$1") ;;
    *.s) args+=(-x assembler-with-cpp "$1") ;;
  esac
  shift
done
exec @llvm@/bin/clang -DCFG_TARGET_OS_windows -DCFG_TARGET_ENV_msvc "-DCFG_TARGET_ARCH_$arch" "${args[@]}"
