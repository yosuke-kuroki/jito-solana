#!/bin/bash -e

cd "$(dirname "$0")/.."

if [[ -z "$BUILDKITE_TAG" ]]; then
  # Skip publish if this is not a tagged release
  exit 0
fi

if [[ -z "$CRATES_IO_TOKEN" ]]; then
  echo CRATES_IO_TOKEN undefined
  exit 1
fi

# TODO: Ensure the published version matches the contents of BUILDKITE_TAG
ci/docker-run.sh rust \
  bash -exc "cargo package; cargo publish --token $CRATES_IO_TOKEN"

exit 0
