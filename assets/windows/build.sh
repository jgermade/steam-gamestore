#!/bin/sh
# Rebuild depth16.exe with LLVM alone: clang, lld-link and llvm-dlltool.
set -eu
cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
for lib in user32 kernel32; do
    llvm-dlltool -m i386:x86-64 -d "$lib.def" -l "$work/$lib.lib"
done
clang --target=x86_64-pc-windows-msvc -Os -ffreestanding -fno-stack-protector -fno-builtin \
    -c depth16.c -o "$work/depth16.obj"
lld-link /nologo /subsystem:windows /entry:start /nodefaultlib /out:depth16.exe \
    "$work/depth16.obj" "$work/user32.lib" "$work/kernel32.lib"
