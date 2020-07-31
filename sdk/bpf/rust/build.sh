#!/usr/bin/env bash

if [ "$#" -ne 1 ]; then
    echo "Error: Must provide the full path to the project to build"
    exit 1
fi
if [ ! -f "$1/Cargo.toml" ]; then
      echo "Error: Cannot find project: $1"
    exit 1
fi

echo "Building $1"
set -e

bpf_sdk=$(cd "$(dirname "$0")/.." && pwd)

# Ensure the sdk is installed
"$bpf_sdk"/scripts/install.sh

# Use the SDK's version of llvm to build the compiler-builtins for BPF
export CC="$bpf_sdk/dependencies/llvm-native/bin/clang"
export AR="$bpf_sdk/dependencies/llvm-native/bin/llvm-ar"

# Use the SDK's version of Rust to build for BPF
export RUSTUP_TOOLCHAIN=bpf
export RUSTFLAGS="
    -C lto=no \
    -C opt-level=2 \
    -C link-arg=-z -C link-arg=notext \
    -C link-arg=-T$bpf_sdk/rust/bpf.ld \
    -C link-arg=--Bdynamic \
    -C link-arg=-shared \
    -C link-arg=--entry=entrypoint \
    -C link-arg=-no-threads \
    -C linker=$bpf_sdk/dependencies/llvm-native/bin/ld.lld"

# Setup xargo
export XARGO_HOME="$bpf_sdk/dependencies/xargo"
export XARGO_RUST_SRC="$bpf_sdk/dependencies/rust-bpf-sysroot/src"
export RUST_COMPILER_RT_ROOT="$bpf_sdk/dependencies/rust-bpf-sysroot/src/compiler-rt"

cd "$1"
xargo build --target bpfel-unknown-unknown --release --no-default-features --features program

{ { set +x; } 2>/dev/null; echo Success; }
