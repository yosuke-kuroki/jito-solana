#!/usr/bin/env bash
set -e
cd "$(dirname "$0")/.."
source ci/semver_bash/semver.sh

# List of internal crates to publish
#
# IMPORTANT: the order of the CRATES *is* significant.  Crates must be published
# before the crates that depend on them.  Note that this information is already
# expressed in the various Cargo.toml files, and ideally would not be duplicated
# here. (TODO: figure the crate ordering dynamically)
#
CRATES=(
  kvstore
  logger
  netutil
  sdk
  keygen
  metrics
  client
  drone
  programs/{budget_api,config_api,storage_api,token_api,vote_api}
  runtime
  programs/{budget,bpf_loader,config,vote,rewards,storage,token,vote}
  vote-signer
  core
  fullnode
  genesis
  ledger-tool
  wallet
  install
)


# Only package/publish if this is a tagged release
[[ -n $TRIGGERED_BUILDKITE_TAG ]] || {
  echo TRIGGERED_BUILDKITE_TAG unset, skipped
  exit 0
}

semverParseInto "$TRIGGERED_BUILDKITE_TAG" MAJOR MINOR PATCH SPECIAL
expectedCrateVersion="$MAJOR.$MINOR.$PATCH$SPECIAL"

[[ -n "$CRATES_IO_TOKEN" ]] || {
  echo CRATES_IO_TOKEN undefined
  exit 1
}

cargoCommand="cargo publish --token $CRATES_IO_TOKEN"

for crate in "${CRATES[@]}"; do
  if [[ ! -r $crate/Cargo.toml ]]; then
    echo "Error: $crate/Cargo.toml does not exist"
    exit 1
  fi
  echo "-- $crate"
  grep -q "^version = \"$expectedCrateVersion\"$" Cargo.toml || {
    echo "Error: $crate/Cargo.toml version is not $expectedCrateVersion"
    exit 1
  }

  (
    set -x
    # TODO: the rocksdb package does not build with the stock rust docker image,
    # so use the solana rust docker image until this is resolved upstream
    source ci/rust-version.sh
    ci/docker-run.sh "$rust_stable_docker_image" bash -exc "cd $crate; $cargoCommand"
    #ci/docker-run.sh rust bash -exc "cd $crate; $cargoCommand"
  )
done

exit 0
