#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"

cargo_install_unless() {
  declare crate=$1
  shift

  "$@" > /dev/null 2>&1 || \
    cargo install "$crate"
}

export PATH=$CARGO_HOME/bin:$PATH
cargo_install_unless mdbook mdbook --help
cargo_install_unless svgbob_cli svgbob --help

make -j"$(nproc)"
