#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"/..

if [[ -z $BUILDKITE ]]; then
  echo BUILDKITE not defined
  exit 1
fi

if [[ -z $SOLANA_METRICS_PARTIAL_CONFIG ]]; then
  echo SOLANA_METRICS_PARTIAL_CONFIG not defined
  exit 1
fi

if [[ -z $TESTNET ]]; then
  TESTNET=$(buildkite-agent meta-data get "testnet" --default "")
fi

if [[ -z $TESTNET_OP ]]; then
  TESTNET_OP=$(buildkite-agent meta-data get "testnet-operation" --default "")
fi

if [[ -z $TESTNET || -z $TESTNET_OP ]]; then
  (
    cat <<EOF
steps:
  - block: "Manage Testnet"
    fields:
      - select: "Network"
        key: "testnet"
        options:
          - label: "testnet"
            value: "testnet"
          - label: "testnet-perf"
            value: "testnet-perf"
          - label: "testnet-edge"
            value: "testnet-edge"
          - label: "testnet-edge-perf"
            value: "testnet-edge-perf"
          - label: "testnet-beta"
            value: "testnet-beta"
          - label: "testnet-beta-perf"
            value: "testnet-beta-perf"
      - select: "Operation"
        key: "testnet-operation"
        default: "sanity-or-restart"
        options:
          - label: "Sanity check.  Restart network on failure"
            value: "sanity-or-restart"
          - label: "Start (or restart) the network"
            value: "start"
          - label: "Update the network software.  Restart network on failure"
            value: "update-or-restart"
          - label: "Stop the network"
            value: "stop"
          - label: "Sanity check only"
            value: "sanity"
  - command: "ci/$(basename "$0")"
    agents:
      - "queue=$BUILDKITE_AGENT_META_DATA_QUEUE"
EOF
  ) | buildkite-agent pipeline upload
  exit 0
fi

if [[ -n $TESTNET_DB_HOST ]]; then
  SOLANA_METRICS_PARTIAL_CONFIG="host=$TESTNET_DB_HOST,$SOLANA_METRICS_PARTIAL_CONFIG"
fi

export SOLANA_METRICS_CONFIG="db=$TESTNET,$SOLANA_METRICS_PARTIAL_CONFIG"
echo "SOLANA_METRICS_CONFIG: $SOLANA_METRICS_CONFIG"
source scripts/configure-metrics.sh

ci/channel-info.sh
eval "$(ci/channel-info.sh)"

if [[ -n $TESTNET_TAG ]]; then
  CHANNEL_OR_TAG=$TESTNET_TAG
else
  case $TESTNET in
  testnet-edge|testnet-edge-perf)
    CHANNEL_OR_TAG=edge
    CHANNEL_BRANCH=$EDGE_CHANNEL
    ;;
  testnet-beta|testnet-beta-perf)
    CHANNEL_OR_TAG=beta
    CHANNEL_BRANCH=$BETA_CHANNEL
    ;;
  testnet|testnet-perf)
    CHANNEL_OR_TAG=$STABLE_CHANNEL_LATEST_TAG
    CHANNEL_BRANCH=$STABLE_CHANNEL
    ;;
  *)
    echo "Error: Invalid TESTNET=$TESTNET"
    exit 1
    ;;
  esac

  if [[ $BUILDKITE_BRANCH != "$CHANNEL_BRANCH" ]]; then
    (
      cat <<EOF
steps:
  - trigger: "$BUILDKITE_PIPELINE_SLUG"
    async: true
    build:
      message: "$BUILDKITE_MESSAGE"
      branch: "$CHANNEL_BRANCH"
      env:
        TESTNET: "$TESTNET"
        TESTNET_OP: "$TESTNET_OP"
        TESTNET_DB_HOST: "$TESTNET_DB_HOST"
        EC2_NODE_COUNT: "$EC2_NODE_COUNT"
        GCE_NODE_COUNT: "$GCE_NODE_COUNT"
EOF
    ) | buildkite-agent pipeline upload
    exit 0
  fi
fi

sanity() {
  echo "--- sanity $TESTNET"
  case $TESTNET in
  testnet-edge)
    (
      set -x
      ci/testnet-sanity.sh edge-testnet-solana-com ec2 us-west-1a
    )
    ;;
  testnet-edge-perf)
    (
      set -x
      REJECT_EXTRA_NODES=1 \
      NO_LEDGER_VERIFY=1 \
      NO_VALIDATOR_SANITY=1 \
        ci/testnet-sanity.sh edge-perf-testnet-solana-com ec2 us-west-2b
    )
    ;;
  testnet-beta)
    (
      set -x
      EC2_ZONES=(us-west-1a sa-east-1a ap-northeast-2a eu-central-1a ca-central-1a)
      ok=true
      for zone in "${EC2_ZONES[@]}"; do
        if ! $ok; then
          break
        fi
        ci/testnet-sanity.sh beta-testnet-solana-com ec2 "$zone" || ok=false
      done

      GCE_ZONES=(us-west1-b asia-east2-a europe-west4-a southamerica-east1-b us-east4-c)
      for zone in "${GCE_ZONES[@]}"; do
        if ! $ok; then
          break
        fi
        ci/testnet-sanity.sh beta-testnet-solana-com gce "$zone" || ok=false
      done
      $ok
    )
    ;;
  testnet-beta-perf)
    (
      set -x
      REJECT_EXTRA_NODES=1 \
      NO_LEDGER_VERIFY=1 \
      NO_VALIDATOR_SANITY=1 \
        ci/testnet-sanity.sh beta-perf-testnet-solana-com ec2 us-west-2b
    )
    ;;
  testnet)
    (
      set -x
      ci/testnet-sanity.sh testnet-solana-com ec2 us-west-1a
      #ci/testnet-sanity.sh testnet-solana-com gce us-east1-c
    )
    ;;
  testnet-perf)
    (
      set -x
      REJECT_EXTRA_NODES=1 \
      NO_LEDGER_VERIFY=1 \
      NO_VALIDATOR_SANITY=1 \
        ci/testnet-sanity.sh perf-testnet-solana-com gce us-west1-b
      #ci/testnet-sanity.sh perf-testnet-solana-com ec2 us-east-1a
    )
    ;;
  *)
    echo "Error: Invalid TESTNET=$TESTNET"
    exit 1
    ;;
  esac
}


start() {
  declare maybeDelete=$1
  if [[ -z $maybeDelete ]]; then
    echo "--- start $TESTNET"
  else
    echo "--- stop $TESTNET"
  fi
  declare maybeReuseLedger=$2

  case $TESTNET in
  testnet-edge)
    (
      set -x
      NO_VALIDATOR_SANITY=1 \
        ci/testnet-deploy.sh -p edge-testnet-solana-com -C ec2 -z us-west-1a \
          -t "$CHANNEL_OR_TAG" -n 3 -c 0 -u -P -a eipalloc-0ccd4f2239886fa94 \
          ${maybeReuseLedger:+-r} \
          ${maybeDelete:+-D}
    )
    ;;
  testnet-edge-perf)
    (
      set -x
      NO_LEDGER_VERIFY=1 \
      NO_VALIDATOR_SANITY=1 \
      RUST_LOG=solana=warn \
        ci/testnet-deploy.sh -p edge-perf-testnet-solana-com -C ec2 -z us-west-2b \
          -g -t "$CHANNEL_OR_TAG" -c 2 \
          -b \
          ${maybeReuseLedger:+-r} \
          ${maybeDelete:+-D}
    )
    ;;
  testnet-beta)
    (
      set -x
      EC2_ZONES=(us-west-1a sa-east-1a ap-northeast-2a eu-central-1a ca-central-1a)
      GCE_ZONES=(us-west1-b asia-east2-a europe-west4-a southamerica-east1-b us-east4-c)

      # Build an array to pass as opts to testnet-deploy.sh: "-z zone1 -z zone2 ..."
      GCE_ZONE_ARGS=()
      for val in "${GCE_ZONES[@]}"; do
        GCE_ZONE_ARGS+=("-z $val")
      done

      EC2_ZONE_ARGS=()
      for val in "${EC2_ZONES[@]}"; do
        EC2_ZONE_ARGS+=("-z $val")
      done

      [[ -n $EC2_NODE_COUNT ]] || EC2_NODE_COUNT=60
      [[ -n $GCE_NODE_COUNT ]] || GCE_NODE_COUNT=40

      # shellcheck disable=SC2068
      NO_VALIDATOR_SANITY=1 \
        ci/testnet-deploy.sh -p beta-testnet-solana-com -C ec2 ${EC2_ZONE_ARGS[@]} \
          -t "$CHANNEL_OR_TAG" -n "$EC2_NODE_COUNT" -c 0 -s -u -P -a eipalloc-0f286cf8a0771ce35 \
          ${maybeReuseLedger:+-r} \
          ${maybeDelete:+-D}
      # shellcheck disable=SC2068
      NO_VALIDATOR_SANITY=1 \
        ci/testnet-deploy.sh -p beta-testnet-solana-com -C gce ${GCE_ZONE_ARGS[@]} \
          -t "$CHANNEL_OR_TAG" -n "$GCE_NODE_COUNT" -c 0 -x -P \
          ${maybeReuseLedger:+-r} \
          ${maybeDelete:+-D}
    )
    ;;
  testnet-beta-perf)
    (
      set -x
      NO_LEDGER_VERIFY=1 \
      NO_VALIDATOR_SANITY=1 \
      RUST_LOG=solana=warn \
        ci/testnet-deploy.sh -p beta-perf-testnet-solana-com -C ec2 -z us-west-2b \
          -g -t "$CHANNEL_OR_TAG" -c 2 \
          -b \
          ${maybeReuseLedger:+-r} \
          ${maybeDelete:+-D}
    )
    ;;
  testnet)
    (
      set -x
      NO_VALIDATOR_SANITY=1 \
        ci/testnet-deploy.sh -p testnet-solana-com -C ec2 -z us-west-1a \
          -t "$CHANNEL_OR_TAG" -n 3 -c 0 -u -P -a eipalloc-0fa502bf95f6f18b2 \
          -b \
          ${maybeReuseLedger:+-r} \
          ${maybeDelete:+-D}
        #ci/testnet-deploy.sh -p testnet-solana-com -C gce -z us-east1-c \
        #  -t "$CHANNEL_OR_TAG" -n 3 -c 0 -P -a testnet-solana-com  \
        #  ${maybeReuseLedger:+-r} \
        #  ${maybeDelete:+-D}
    )
    ;;
  testnet-perf)
    (
      set -x
      NO_LEDGER_VERIFY=1 \
      NO_VALIDATOR_SANITY=1 \
      RUST_LOG=solana=warn \
        ci/testnet-deploy.sh -p perf-testnet-solana-com -C gce -z us-west1-b \
          -G "n1-standard-16 --accelerator count=2,type=nvidia-tesla-v100" \
          -t "$CHANNEL_OR_TAG" -c 2 \
          -b \
          -d pd-ssd \
          ${maybeReuseLedger:+-r} \
          ${maybeDelete:+-D}
        #ci/testnet-deploy.sh -p perf-testnet-solana-com -C ec2 -z us-east-1a \
        #  -g \
        #  -t "$CHANNEL_OR_TAG" -c 2 \
        #  ${maybeReuseLedger:+-r} \
        #  ${maybeDelete:+-D}
    )
    ;;
  *)
    echo "Error: Invalid TESTNET=$TESTNET"
    exit 1
    ;;
  esac
}

stop() {
  start delete
}

case $TESTNET_OP in
sanity)
  sanity
  ;;
start)
  start
  ;;
stop)
  stop
  ;;
update-or-restart)
  if start "" update; then
    echo Update successful
  else
    echo "+++ Update failed, restarting the network"
    $metricsWriteDatapoint "testnet-manager update-failure=1"
    start
  fi
  ;;
sanity-or-restart)
  if sanity; then
    echo Pass
  else
    echo "+++ Sanity failed, updating the network"
    $metricsWriteDatapoint "testnet-manager sanity-failure=1"
    if start "" update; then
      echo Update successful
    else
      echo "+++ Update failed, restarting the network"
      $metricsWriteDatapoint "testnet-manager update-failure=1"
      start
    fi
  fi
  ;;
esac

echo --- fin
exit 0
