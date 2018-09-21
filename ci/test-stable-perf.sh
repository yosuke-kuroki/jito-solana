#!/bin/bash -e

cd "$(dirname "$0")/.."

if ! ci/version-check.sh stable; then
  # This job doesn't run within a container, try once to upgrade tooling on a
  # version check failure
  rustup install stable
  ci/version-check.sh stable
fi
export RUST_BACKTRACE=1
export RUSTFLAGS="-D warnings"

./fetch-perf-libs.sh
export LD_LIBRARY_PATH=$PWD/target/perf-libs:/usr/local/cuda/lib64:$LD_LIBRARY_PATH
export PATH=$PATH:/usr/local/cuda/bin

_() {
  echo "--- $*"
  "$@"
}

_ cargo test --features=cuda,erasure

echo --- ci/localnet-sanity.sh
(
  set -x
  # Assume |cargo build| has populated target/debug/ successfully.
  export PATH=$PWD/target/debug:$PATH
  USE_INSTALL=1 ci/localnet-sanity.sh
)
