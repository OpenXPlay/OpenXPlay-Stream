#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 OpenXPlay.org
# SPDX-License-Identifier: AGPL-3.0-only
# Created by riko chen on 2026-10-14
#
# Builds static aarch64 Linux (musl) release binaries on macOS or Linux hosts.
# ring compiles C code, which needs musl headers, clang and an ELF-aware archiver.
# Requires: rustup target add aarch64-unknown-linux-musl; rustup component add llvm-tools; clang.
set -euo pipefail
cd "$(dirname "$0")/.."

MUSL_VERSION=1.2.5
MUSL_SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
SYSROOT="$PWD/target/musl-sysroot-aarch64"

if [ ! -f "$SYSROOT/include/stddef.h" ]; then
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  curl -sSfL -o "$tmp/musl.tar.gz" "https://musl.libc.org/releases/musl-$MUSL_VERSION.tar.gz"
  if command -v sha256sum >/dev/null; then
    echo "$MUSL_SHA256  $tmp/musl.tar.gz" | sha256sum -c - >/dev/null
  else
    echo "$MUSL_SHA256  $tmp/musl.tar.gz" | shasum -a 256 -c - >/dev/null
  fi
  tar xzf "$tmp/musl.tar.gz" -C "$tmp"
  make -s -C "$tmp/musl-$MUSL_VERSION" ARCH=aarch64 prefix="$SYSROOT" install-headers
fi

AR=$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-ar 2>/dev/null | head -n 1 || true)
if [ ! -x "$AR" ]; then
  echo "llvm-ar not found; run: rustup component add llvm-tools" >&2
  exit 1
fi

export CC_aarch64_unknown_linux_musl=clang
export CFLAGS_aarch64_unknown_linux_musl="--target=aarch64-unknown-linux-musl --sysroot=$SYSROOT -isystem $SYSROOT/include"
export AR_aarch64_unknown_linux_musl="$AR"
exec cargo build --release --target aarch64-unknown-linux-musl "$@"
