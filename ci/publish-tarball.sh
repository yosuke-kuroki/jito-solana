#!/usr/bin/env bash
set -e

cd "$(dirname "$0")/.."

if [[ -n $APPVEYOR ]]; then
  # Bootstrap rust build environment
  source ci/env.sh
  source ci/rust-version.sh

  appveyor DownloadFile https://win.rustup.rs/ -FileName rustup-init.exe
  export USERPROFILE="D:\\"
  ./rustup-init -yv --default-toolchain $rust_stable --default-host x86_64-pc-windows-msvc
  export PATH="$PATH:/d/.cargo/bin"
  rustc -vV
  cargo -vV
fi

DRYRUN=
if [[ -z $CI_BRANCH ]]; then
  DRYRUN="echo"
  CHANNEL=unknown
fi

eval "$(ci/channel-info.sh)"

TAG=
if [[ -n "$CI_TAG" ]]; then
  CHANNEL_OR_TAG=$CI_TAG
  TAG="$CI_TAG"
else
  CHANNEL_OR_TAG=$CHANNEL
fi

if [[ -z $CHANNEL_OR_TAG ]]; then
  echo +++ Unable to determine channel to publish into, exiting.
  exit 0
fi

case "$CI_OS_NAME" in
osx)
  TARGET=x86_64-apple-darwin
  ;;
linux)
  TARGET=x86_64-unknown-linux-gnu
  ;;
windows)
  TARGET=x86_64-pc-windows-msvc
  ;;
*)
  echo CI_OS_NAME unset
  exit 1
  ;;
esac

echo --- Creating tarball
(
  set -x
  rm -rf solana-release/
  mkdir solana-release/

  COMMIT="$(git rev-parse HEAD)"

  (
    echo "channel: $CHANNEL_OR_TAG"
    echo "commit: $COMMIT"
    echo "target: $TARGET"
  ) > solana-release/version.yml

  source ci/rust-version.sh stable
  scripts/cargo-install-all.sh +"$rust_stable" solana-release

  # Reduce the Windows archive size until
  # https://github.com/appveyor/ci/issues/2997 is fixed
  if [[ -n $APPVEYOR ]]; then
    rm -f \
      solana-release/bin/solana-validator.exe \
      solana-release/bin/solana-bench-exchange.exe \

  fi

  tar cvf solana-release-$TARGET.tar solana-release
  bzip2 solana-release-$TARGET.tar
  cp solana-release/bin/solana-install-init solana-install-init-$TARGET
  cp solana-release/version.yml solana-release-$TARGET.yml
)

# Metrics tarball is platform agnostic, only publish it from Linux
MAYBE_METRICS_TARBALL=
if [[ "$CI_OS_NAME" = linux ]]; then
  metrics/create-metrics-tarball.sh
  MAYBE_METRICS_TARBALL=solana-metrics.tar.bz2
fi

source ci/upload-ci-artifact.sh

for file in solana-release-$TARGET.tar.bz2 solana-release-$TARGET.yml solana-install-init-"$TARGET"* $MAYBE_METRICS_TARBALL; do
  upload-ci-artifact "$file"

  if [[ -n $DO_NOT_PUBLISH_TAR ]]; then
    echo "Skipped $file due to DO_NOT_PUBLISH_TAR"
    continue
  fi

  if [[ -n $BUILDKITE ]]; then
    echo --- AWS S3 Store: "$file"
    (
      set -x
      $DRYRUN docker run \
        --rm \
        --env AWS_ACCESS_KEY_ID \
        --env AWS_SECRET_ACCESS_KEY \
        --volume "$PWD:/solana" \
        eremite/aws-cli:2018.12.18 \
        /usr/bin/s3cmd --acl-public put /solana/"$file" s3://release.solana.com/"$CHANNEL_OR_TAG"/"$file"

      echo Published to:
      $DRYRUN ci/format-url.sh http://release.solana.com/"$CHANNEL_OR_TAG"/"$file"
    )

    if [[ -n $TAG ]]; then
      ci/upload-github-release-asset.sh "$file"
    fi
  elif [[ -n $TRAVIS ]]; then
    # .travis.yml uploads everything in the travis-s3-upload/ directory to release.solana.com
    mkdir -p travis-s3-upload/"$CHANNEL_OR_TAG"
    cp -v "$file" travis-s3-upload/"$CHANNEL_OR_TAG"/

    if [[ -n $TAG ]]; then
      # .travis.yaml uploads everything in the travis-release-upload/ directory to
      # the associated Github Release
      mkdir -p travis-release-upload/
      cp -v "$file" travis-release-upload/
    fi
  elif [[ -n $APPVEYOR ]]; then
    # Add artifacts for .appveyor.yml to upload
    appveyor PushArtifact "$file" -FileName "$CHANNEL_OR_TAG"/"$file"
  fi
done

echo --- ok
