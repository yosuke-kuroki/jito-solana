#!/bin/bash
#
# Start a validator node
#
here=$(dirname "$0")
# shellcheck source=multinode-demo/common.sh
source "$here"/common.sh

# shellcheck source=scripts/oom-score-adj.sh
source "$here"/../scripts/oom-score-adj.sh

if [[ -d "$SNAP" ]]; then
  # Exit if mode is not yet configured
  # (typically the case after the Snap is first installed)
  [[ -n "$(snapctl get mode)" ]] || exit 0
fi

usage() {
  if [[ -n $1 ]]; then
    echo "$*"
    echo
  fi
  echo "usage: $0 [-x] [rsync network path to leader] [network entry point]"
  echo
  echo " Start a validator on the specified network"
  echo
  echo "   -x: runs a new, dynamically-configured validator"
  echo
  exit 1
}

if [[ $1 = -h ]]; then
  usage
fi

if [[ $1 == -x ]]; then
  self_setup=1
  shift
else
  self_setup=0
fi

if [[ -n $3 ]]; then
  usage
fi

read -r leader leader_address shift < <(find_leader "${@:1:2}")
shift "$shift"

if [[ -n $SOLANA_CUDA ]]; then
  program=$solana_fullnode_cuda
else
  program=$solana_fullnode
fi

if ((!self_setup)); then
  [[ -f $SOLANA_CONFIG_VALIDATOR_DIR/validator.json ]] || {
    echo "$SOLANA_CONFIG_VALIDATOR_DIR/validator.json not found, create it by running:"
    echo
    echo "  ${here}/setup.sh"
    exit 1
  }
  validator_json_path=$SOLANA_CONFIG_VALIDATOR_DIR/validator.json
  SOLANA_LEADER_CONFIG_DIR=$SOLANA_CONFIG_VALIDATOR_DIR/leader-config
else
  mkdir -p "$SOLANA_CONFIG_PRIVATE_DIR"
  validator_id_path=$SOLANA_CONFIG_PRIVATE_DIR/validator-id-x$$.json
  $solana_keygen -o "$validator_id_path"

  mkdir -p "$SOLANA_CONFIG_VALIDATOR_DIR"
  validator_json_path=$SOLANA_CONFIG_VALIDATOR_DIR/validator-x$$.json

  port=9000
  (((port += ($$ % 1000)) && (port == 9000) && port++))

  $solana_fullnode_config --keypair="$validator_id_path" -l -b "$port" > "$validator_json_path"

  SOLANA_LEADER_CONFIG_DIR=$SOLANA_CONFIG_VALIDATOR_DIR/leader-config-x$$
fi

rsync_leader_url=$(rsync_url "$leader")

tune_networking

set -ex
$rsync -vPr "$rsync_leader_url"/config/ "$SOLANA_LEADER_CONFIG_DIR"
[[ -d $SOLANA_LEADER_CONFIG_DIR/ledger ]] || {
  echo "Unable to retrieve ledger from $rsync_leader_url"
  exit 1
}

trap 'kill "$pid" && wait "$pid"' INT TERM
$program \
  --identity "$validator_json_path" \
  --network "$leader_address" \
  --ledger "$SOLANA_LEADER_CONFIG_DIR"/ledger \
  > >($validator_logger) 2>&1 &
pid=$!
oom_score_adj "$pid" 1000
wait "$pid"
