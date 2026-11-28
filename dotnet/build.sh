#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Builds the braga_ffi cdylib, stages it under runtimes/<rid>/native/ (the NuGet convention), then
# runs the xUnit conformance tests and builds the package. Artifacts are generated, not committed.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"

export PATH="${DOTNET_ROOT:-$HOME/.dotnet}:$HOME/.cargo/bin:$PATH"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) rid=osx-arm64; lib=libbraga_ffi.dylib ;;
  Darwin-x86_64) rid=osx-x64; lib=libbraga_ffi.dylib ;;
  Linux-x86_64) rid=linux-x64; lib=libbraga_ffi.so ;;
  Linux-aarch64) rid=linux-arm64; lib=libbraga_ffi.so ;;
  *)
    echo "build.sh: unsupported platform $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

cargo build --release -p braga-ffi --manifest-path "$repo/Cargo.toml"
mkdir -p "$here/runtimes/$rid/native"
cp "$repo/target/release/$lib" "$here/runtimes/$rid/native/$lib"

cd "$here"
dotnet test
dotnet pack src/Braga/Braga.csproj -c Release
