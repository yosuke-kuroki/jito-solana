#!/bin/bash -e

cd "$(dirname "$0")/.."
source ci/upload_ci_artifact.sh

ci/version-check.sh nightly
export RUST_BACKTRACE=1

_() {
  echo "--- $*"
  "$@"
}

# Uncomment this to run nightly test suit
# _ cargo test --verbose --features=unstable

maybe_cargo_install() {
  for cmd in "$@"; do
    set +e
    cargo "$cmd" --help > /dev/null 2>&1
    declare exitcode=$?
    set -e
    if [[ $exitcode -eq 101 ]]; then
      _ cargo install cargo-"$cmd"
    fi
  done
}

maybe_cargo_install cov

# Generate coverage data and report via unit-test suite.
_ cargo cov clean
_ cargo cov test --lib
_ cargo cov report

# Generate a coverage report with grcov via lcov.
if [[ ! -f ./grcov ]]; then
  uname=$(uname | tr '[:upper:]' '[:lower:]')
  uname_m=$(uname -m | tr '[:upper:]' '[:lower:]')
  name=grcov-${uname}-${uname_m}.tar.bz2
  _ wget "https://github.com/mozilla/grcov/releases/download/v0.2.3/${name}"
  _ tar -xjf "${name}"
fi
_ ./grcov . -t lcov > lcov.info
_ genhtml -o target/cov/report-lcov --show-details --highlight --ignore-errors source --legend lcov.info

# Upload to tarballs to buildkite.
_ cd target/cov && tar -cjf cov-report.tar.bz2 report/* && cd -
_ upload_ci_artifact "target/cov/cov-report.tar.bz2"

_ cd target/cov && tar -cjf lcov-report.tar.bz2 report-lcov/* && cd -
_ upload_ci_artifact "target/cov/lcov-report.tar.bz2"

if [[ -z "$CODECOV_TOKEN" ]]; then
  echo CODECOV_TOKEN undefined
else
  true
  # TODO: Why doesn't codecov grok our lcov files?
  #bash <(curl -s https://codecov.io/bash) -X gcov
fi
