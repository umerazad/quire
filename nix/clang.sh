#!/usr/bin/env bash
# `clang` in the macOS dev shell. Windows targets go to LLVM's clang: ring builds ARM64 Windows
# code with plain clang, but cargo-xwin's CFLAGS spell include folders clang-cl's way
# (`/imsvc <dir>`), so those become `-isystem <dir>`. Every other call goes to Xcode's clang.
case " $* " in
  *windows-msvc*) ;;
  *) exec /usr/bin/clang "$@" ;;
esac
args=()
for arg in "$@"; do
  if [ "$arg" = /imsvc ]; then args+=(-isystem); else args+=("$arg"); fi
done
exec @llvm@/bin/clang "${args[@]}"
