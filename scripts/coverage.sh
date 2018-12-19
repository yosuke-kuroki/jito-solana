#!/usr/bin/env bash
#
# Runs all tests and collects code coverage
#
# Warning: this process is a little slow
#

set -e
cd "$(dirname "$0")/.."
source ci/_

: "${BUILDKITE_COMMIT:=local}"
reportName="lcov-${BUILDKITE_COMMIT:0:9}"

coverageFlags=(-Zprofile)                # Enable coverage
coverageFlags+=("-Clink-dead-code")      # Dead code should appear red in the report
coverageFlags+=("-Ccodegen-units=1")     # Disable ThinLTO which corrupts debuginfo (see [rustc issue #45511]).
coverageFlags+=("-Cinline-threshold=0")  # Disable inlining, which complicates control flow.
coverageFlags+=("-Coverflow-checks=off") # Disable overflow checks, which create unnecessary branches.

export RUSTFLAGS="${coverageFlags[*]}"
export CARGO_INCREMENTAL=0
export RUST_BACKTRACE=1

echo "--- remove old coverage results"
if [[ -d target/cov ]]; then
  find target/cov -name \*.gcda -print0 | xargs -0 rm -f
fi
rm -rf target/cov/$reportName

_ cargo +nightly build --target-dir target/cov --all
_ cargo +nightly test --target-dir target/cov --lib --all -- --test-threads=1

_ scripts/fetch-grcov.sh
echo "--- grcov"
./grcov target/cov/debug/deps/ > target/cov/lcov-full.info

echo "--- filter-non-local-files-from-lcov"
# TODO: The grcov `-s` option could be used to replace this function once grcov
# doesn't panic on files with the same name in different directories of a
# repository
filter-non-local-files-from-lcov() {
  declare skip=false
  while read -r line; do
    if [[ $line =~ ^SF:/ ]]; then
      skip=true # Skip all absolute paths as these are references into ~/.cargo
    elif [[ $line =~ ^SF:(.*) ]]; then
      # Skip relative paths that don't exist
      declare file="${BASH_REMATCH[1]}"
      if [[ -r $file ]]; then
        skip=false
      else
        skip=true
      fi
    fi
    [[ $skip = true ]] || echo "$line"
  done
}

filter-non-local-files-from-lcov < target/cov/lcov-full.info > target/cov/lcov.info

echo "--- html report"
# ProTip: genhtml comes from |brew install lcov| or |apt-get install lcov|
genhtml --output-directory target/cov/$reportName \
  --show-details \
  --highlight \
  --ignore-errors source \
  --prefix "$PWD" \
  --legend \
  target/cov/lcov.info

(
  cd target/cov
  tar zcf report.tar.gz $reportName
)

ls -l target/cov/$reportName/index.html
