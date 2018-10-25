#!/bin/bash -e

cd "$(dirname "$0")/.."

if ! ci/version-check.sh stable; then
  # This job doesn't run within a container, try once to upgrade tooling on a
  # version check failure
  rustup install stable
  ci/version-check.sh stable
fi

DRYRUN=
if [[ -z $BUILDKITE_BRANCH ]] || ./ci/is-pr.sh; then
  DRYRUN="echo"
fi

eval "$(ci/channel-info.sh)"

if [[ $BUILDKITE_BRANCH = "$STABLE_CHANNEL" ]]; then
  CHANNEL=stable
elif [[ $BUILDKITE_BRANCH = "$EDGE_CHANNEL" ]]; then
  CHANNEL=edge
elif [[ $BUILDKITE_BRANCH = "$BETA_CHANNEL" ]]; then
  CHANNEL=beta
fi

if [[ -z $CHANNEL ]]; then
  echo Unable to determine channel to publish into, exiting.
  exit 0
fi

if [[ -z $DRYRUN ]]; then
  [[ -n $SNAPCRAFT_CREDENTIALS_KEY ]] || {
    echo SNAPCRAFT_CREDENTIALS_KEY not defined
    exit 1;
  }
  (
    openssl aes-256-cbc -d \
      -in ci/snapcraft.credentials.enc \
      -out ci/snapcraft.credentials \
      -k "$SNAPCRAFT_CREDENTIALS_KEY"

    snapcraft login --with ci/snapcraft.credentials
  ) || {
    rm -f ci/snapcraft.credentials;
    exit 1
  }
fi

set -x

echo --- checking for multilog
if [[ ! -x /usr/bin/multilog ]]; then
  if [[ -z $CI ]]; then
    echo "multilog not found, install with: sudo apt-get install -y daemontools"
    exit 1
  fi
  sudo apt-get install -y daemontools
fi

echo --- build: $CHANNEL channel
snapcraft

source ci/upload_ci_artifact.sh
upload_ci_artifact solana_*.snap

if [[ -z $DO_NOT_PUBLISH_SNAP ]]; then
  echo --- publish: $CHANNEL channel
  $DRYRUN snapcraft push solana_*.snap --release $CHANNEL
fi
