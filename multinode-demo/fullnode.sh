#!/usr/bin/env bash
#
# Start a fullnode
#
here=$(dirname "$0")
# shellcheck source=multinode-demo/common.sh
source "$here"/common.sh

# shellcheck source=scripts/oom-score-adj.sh
source "$here"/../scripts/oom-score-adj.sh


fullnode_usage() {
  if [[ -n $1 ]]; then
    echo "$*"
    echo
  fi
  cat <<EOF

Fullnode Usage:
usage: $0 [--blockstream PATH] [--init-complete-file FILE] [--label LABEL] [--stake LAMPORTS] [--no-voting] [--rpc-port port] [rsync network path to bootstrap leader configuration] [cluster entry point]

Start a validator or a replicator

  --blockstream PATH        - open blockstream at this unix domain socket location
  --init-complete-file FILE - create this file, if it doesn't already exist, once node initialization is complete
  --label LABEL             - Append the given label to the configuration files, useful when running
                              multiple fullnodes in the same workspace
  --stake LAMPORTS          - Number of lamports to stake
  --no-voting               - start node without vote signer
  --rpc-port port           - custom RPC port for this node
  --no-restart              - do not restart the node if it exits
  --no-airdrop              - The genesis block has an account for the node. Airdrops are not required.

EOF
  exit 1
}

find_entrypoint() {
  declare entrypoint entrypoint_address
  declare shift=0

  if [[ -z $1 ]]; then
    entrypoint="$SOLANA_ROOT"         # Default to local tree for rsync
    entrypoint_address=127.0.0.1:8001 # Default to local entrypoint
  elif [[ -z $2 ]]; then
    entrypoint=$1
    entrypoint_address=$entrypoint:8001
    shift=1
  else
    entrypoint=$1
    entrypoint_address=$2
    shift=2
  fi

  echo "$entrypoint" "$entrypoint_address" "$shift"
}

rsync_url() { # adds the 'rsync://` prefix to URLs that need it
  declare url="$1"

  if [[ $url =~ ^.*:.*$ ]]; then
    # assume remote-shell transport when colon is present, use $url unmodified
    echo "$url"
    return 0
  fi

  if [[ -d $url ]]; then
    # assume local directory if $url is a valid directory, use $url unmodified
    echo "$url"
    return 0
  fi

  # Default to rsync:// URL
  echo "rsync://$url"
}

setup_validator_accounts() {
  declare entrypoint_ip=$1
  declare node_lamports=$2
  declare stake_lamports=$3

  if [[ -f $configured_flag ]]; then
    echo "Vote and stake accounts have already been configured"
  else
    if ((airdrops_enabled)); then
      # Fund the node with enough tokens to fund its Vote, Staking, and Storage accounts
      declare fees=100 # TODO: No hardcoded transaction fees, fetch the current cluster fees
      $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" airdrop $((node_lamports+stake_lamports+fees)) || return $?
    else
      echo "current account balance is "
      $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" balance || return $?
    fi

    # Fund the vote account from the node, with the node as the identity_pubkey
    $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" \
      create-vote-account "$vote_pubkey" "$identity_pubkey" 1 --commission 127 || return $?

    # Fund the stake account from the node, with the node as the identity_pubkey
    $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" \
      create-stake-account "$stake_pubkey" "$stake_lamports" || return $?

    # Delegate the stake.  The transaction fee is paid by the node but the
    #  transaction must be signed by the stake_keypair
    $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" \
      delegate-stake "$stake_keypair_path" "$vote_pubkey" "$stake_lamports" || return $?

    # Setup validator storage account
    $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" \
      create-validator-storage-account "$identity_pubkey" "$storage_pubkey" || return $?

    touch "$configured_flag"
  fi

  $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" \
    show-vote-account "$vote_pubkey"
  $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" \
    show-stake-account "$stake_pubkey"
  $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" \
    show-storage-account "$storage_pubkey"

  echo "Identity account balance:"
  $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" balance
  return 0
}

setup_replicator_account() {
  declare entrypoint_ip=$1
  declare node_lamports=$2

  if [[ -f $configured_flag ]]; then
    echo "Replicator account has already been configured"
  else
    if ((airdrops_enabled)); then
      $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" airdrop "$node_lamports" || return $?
    else
      echo "current account balance is "
      $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" balance || return $?
    fi

    # Setup replicator storage account
    $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" \
      create-replicator-storage-account "$identity_pubkey" "$storage_pubkey" || return $?

    touch "$configured_flag"
  fi

  $solana_wallet --keypair "$identity_keypair_path" --url "http://$entrypoint_ip:8899" \
    show-storage-account "$storage_pubkey"

  return 0
}

ledger_not_setup() {
  echo "Error: $*"
  echo
  echo "Please run: ${here}/setup.sh"
  exit 1
}

args=()
node_type=validator
node_lamports=424242  # number of lamports to assign the node for transaction fees
stake_lamports=42     # number of lamports to assign as stake
poll_for_new_genesis_block=0
label=
identity_keypair_path=
no_restart=0
airdrops_enabled=1
generate_snapshots=0
boot_from_snapshot=1

positional_args=()
while [[ -n $1 ]]; do
  if [[ ${1:0:1} = - ]]; then
    if [[ $1 = --label ]]; then
      label="-$2"
      shift 2
    elif [[ $1 = --no-restart ]]; then
      no_restart=1
      shift
    elif [[ $1 = --bootstrap-leader ]]; then
      node_type=bootstrap_leader
      generate_snapshots=1
      shift
    elif [[ $1 = --generate-snapshots ]]; then
      generate_snapshots=1
      shift
    elif [[ $1 = --no-snapshot ]]; then
      boot_from_snapshot=0
      shift
    elif [[ $1 = --replicator ]]; then
      node_type=replicator
      shift
    elif [[ $1 = --validator ]]; then
      node_type=validator
      shift
    elif [[ $1 = --poll-for-new-genesis-block ]]; then
      poll_for_new_genesis_block=1
      shift
    elif [[ $1 = --blockstream ]]; then
      stake_lamports=0
      args+=("$1" "$2")
      shift 2
    elif [[ $1 = --identity ]]; then
      identity_keypair_path=$2
      args+=("$1" "$2")
      shift 2
    elif [[ $1 = --enable-rpc-exit ]]; then
      args+=("$1")
      shift
    elif [[ $1 = --init-complete-file ]]; then
      args+=("$1" "$2")
      shift 2
    elif [[ $1 = --stake ]]; then
      stake_lamports="$2"
      shift 2
    elif [[ $1 = --no-voting ]]; then
      args+=("$1")
      shift
    elif [[ $1 = --no-sigverify ]]; then
      args+=("$1")
      shift
    elif [[ $1 = --rpc-port ]]; then
      args+=("$1" "$2")
      shift 2
    elif [[ $1 = --dynamic-port-range ]]; then
      args+=("$1" "$2")
      shift 2
    elif [[ $1 = --gossip-port ]]; then
      args+=("$1" "$2")
      shift 2
    elif [[ $1 = --no-airdrop ]]; then
      airdrops_enabled=0
      shift
    elif [[ $1 = -h ]]; then
      fullnode_usage "$@"
    else
      echo "Unknown argument: $1"
      exit 1
    fi
  else
    positional_args+=("$1")
    shift
  fi
done


if [[ $node_type = replicator ]]; then
  if [[ ${#positional_args[@]} -gt 2 ]]; then
    fullnode_usage "$@"
  fi

  read -r entrypoint entrypoint_address shift < <(find_entrypoint "${positional_args[@]}")
  shift "$shift"

  : "${identity_keypair_path:=$SOLANA_CONFIG_DIR/replicator-keypair$label.json}"
  mkdir -p "$SOLANA_CONFIG_DIR"
  [[ -r "$identity_keypair_path" ]] || $solana_keygen new -o "$identity_keypair_path"

  storage_keypair_path="$SOLANA_CONFIG_DIR"/replicator-storage-keypair$label.json
  ledger_config_dir=$SOLANA_CONFIG_DIR/replicator-ledger$label
  configured_flag=$SOLANA_CONFIG_DIR/replicator$label.configured

  program=$solana_replicator
  default_arg --entrypoint "$entrypoint_address"
  default_arg --identity "$identity_keypair_path"
  default_arg --storage-keypair "$storage_keypair_path"
  default_arg --ledger "$ledger_config_dir"

  rsync_entrypoint_url=$(rsync_url "$entrypoint")

elif [[ $node_type = bootstrap_leader ]]; then
  if [[ ${#positional_args[@]} -ne 0 ]]; then
    fullnode_usage "Unknown argument: ${positional_args[0]}"
  fi

  [[ -f "$SOLANA_CONFIG_DIR"/bootstrap-leader-keypair.json ]] ||
    ledger_not_setup "$SOLANA_CONFIG_DIR/bootstrap-leader-keypair.json not found"

  $solana_ledger_tool --ledger "$SOLANA_CONFIG_DIR"/bootstrap-leader-ledger verify

  : "${identity_keypair_path:=$SOLANA_CONFIG_DIR/bootstrap-leader-keypair.json}"

  vote_keypair_path="$SOLANA_CONFIG_DIR"/bootstrap-leader-vote-keypair.json
  ledger_config_dir="$SOLANA_CONFIG_DIR"/bootstrap-leader-ledger
  state_dir="$SOLANA_CONFIG_DIR"/bootstrap-leader-state
  stake_keypair_path=$SOLANA_CONFIG_DIR/bootstrap-leader-stake-keypair.json
  storage_keypair_path=$SOLANA_CONFIG_DIR/bootstrap-leader-storage-keypair.json
  configured_flag=$SOLANA_CONFIG_DIR/bootstrap-leader.configured

  default_arg --rpc-port 8899
  if ((airdrops_enabled)); then
    default_arg --rpc-drone-address 127.0.0.1:9900
  fi
  default_arg --gossip-port 8001

elif [[ $node_type = validator ]]; then
  if [[ ${#positional_args[@]} -gt 2 ]]; then
    fullnode_usage "$@"
  fi

  read -r entrypoint entrypoint_address shift < <(find_entrypoint "${positional_args[@]}")
  shift "$shift"

  : "${identity_keypair_path:=$SOLANA_CONFIG_DIR/validator-keypair$label.json}"
  mkdir -p "$SOLANA_CONFIG_DIR"
  [[ -r "$identity_keypair_path" ]] || $solana_keygen new -o "$identity_keypair_path"

  vote_keypair_path=$SOLANA_CONFIG_DIR/validator-vote-keypair$label.json
  ledger_config_dir=$SOLANA_CONFIG_DIR/validator-ledger$label
  state_dir="$SOLANA_CONFIG_DIR"/validator-state$label
  stake_keypair_path=$SOLANA_CONFIG_DIR/validator-stake-keypair$label.json
  storage_keypair_path=$SOLANA_CONFIG_DIR/validator-storage-keypair$label.json
  configured_flag=$SOLANA_CONFIG_DIR/validator$label.configured

  default_arg --entrypoint "$entrypoint_address"
  if ((airdrops_enabled)); then
    default_arg --rpc-drone-address "${entrypoint_address%:*}:9900"
  fi

  rsync_entrypoint_url=$(rsync_url "$entrypoint")
else
  echo "Error: Unknown node_type: $node_type"
  exit 1
fi

identity_pubkey=$($solana_keygen pubkey "$identity_keypair_path")

if [[ $node_type != replicator ]]; then
  accounts_config_dir="$state_dir"/accounts
  snapshot_config_dir="$state_dir"/snapshots

  default_arg --identity "$identity_keypair_path"
  default_arg --voting-keypair "$vote_keypair_path"
  default_arg --storage-keypair "$storage_keypair_path"
  default_arg --ledger "$ledger_config_dir"
  default_arg --accounts "$accounts_config_dir"
  default_arg --snapshot-path "$snapshot_config_dir"

  if [[ -n $SOLANA_CUDA ]]; then
    program=$solana_validator_cuda
  else
    program=$solana_validator
  fi
fi

if [[ -z $CI ]]; then # Skip in CI
  # shellcheck source=scripts/tune-system.sh
  source "$here"/../scripts/tune-system.sh
fi

new_genesis_block() {
  (
    set -x
    $rsync -r "${rsync_entrypoint_url:?}"/config/ledger "$SOLANA_RSYNC_CONFIG_DIR"
  ) || (
    echo "Error: failed to rsync genesis ledger"
  )

  ! diff -q "$SOLANA_RSYNC_CONFIG_DIR"/ledger/genesis.bin "$ledger_config_dir"/genesis.bin >/dev/null 2>&1
}

set -e
PS4="$(basename "$0"): "

pid=
kill_fullnode() {
  if [[ -n $pid ]]; then
    declare _pid=$pid
    pid=
    echo "killing pid $_pid"
    kill "$_pid" || true
    wait "$_pid" || true
    echo "$_pid killed"
  fi
}
trap 'kill_fullnode' INT TERM ERR

while true; do
  if [[ $node_type != bootstrap_leader ]] && new_genesis_block; then
    # If the genesis block has changed remove the now stale ledger and
    # vote/stake/storage keypairs for the node and start all over again
    (
      set -x
      rm -rf "$ledger_config_dir" "$state_dir" "$configured_flag"
    )
    if [[ $node_type = validator ]]; then
      $solana_keygen new -f -o "$vote_keypair_path"
      $solana_keygen new -f -o "$stake_keypair_path"
      $solana_keygen new -f -o "$storage_keypair_path"
    fi
    if [[ $node_type = replicator ]]; then
      $solana_keygen new -f -o "$storage_keypair_path"
    fi
  fi

  if [[ $node_type = replicator ]]; then
    storage_pubkey=$($solana_keygen pubkey "$storage_keypair_path")
    setup_replicator_account "${entrypoint_address%:*}" \
      "$node_lamports"

    cat <<EOF
======================[ $node_type configuration ]======================
replicator pubkey: $identity_pubkey
storage pubkey: $storage_pubkey
ledger: $ledger_config_dir
======================================================================
EOF

  else
    if [[ $node_type = bootstrap_leader && ! -d "$SOLANA_RSYNC_CONFIG_DIR"/ledger ]]; then
      ledger_not_setup "$SOLANA_RSYNC_CONFIG_DIR/ledger does not exist"
    fi

    if [[ ! -d "$ledger_config_dir" ]]; then
      if [[ $node_type = validator ]]; then
        (
          cd "$SOLANA_RSYNC_CONFIG_DIR"

          echo "Rsyncing genesis ledger from ${rsync_entrypoint_url:?}..."
          SECONDS=
          while ! $rsync -Pr "${rsync_entrypoint_url:?}"/config/ledger .; do
            echo "Genesis ledger rsync failed"
            sleep 5
          done
          echo "Fetched genesis ledger in $SECONDS seconds"

          if ((boot_from_snapshot)); then
            SECONDS=
            echo "Rsyncing state snapshot ${rsync_entrypoint_url:?}..."
            if ! $rsync -P "${rsync_entrypoint_url:?}"/config/state.tgz .; then
              echo "State snapshot rsync failed"
              rm -f "$SOLANA_RSYNC_CONFIG_DIR"/state.tgz
              exit
            fi
            echo "Fetched snapshot in $SECONDS seconds"

            SECONDS=
            mkdir -p "$state_dir"
            (
              set -x
              tar -C "$state_dir" -zxf "$SOLANA_RSYNC_CONFIG_DIR"/state.tgz
            )
            echo "Extracted snapshot in $SECONDS seconds"
          fi
        )
      fi

      (
        set -x
        cp -a "$SOLANA_RSYNC_CONFIG_DIR"/ledger/ "$ledger_config_dir"
      )
    fi

    vote_pubkey=$($solana_keygen pubkey "$vote_keypair_path")
    stake_pubkey=$($solana_keygen pubkey "$stake_keypair_path")
    storage_pubkey=$($solana_keygen pubkey "$storage_keypair_path")
    replace_arg --vote-account "$vote_pubkey"

    if [[ $node_type = validator ]] && ((stake_lamports)); then
      setup_validator_accounts "${entrypoint_address%:*}" \
        "$node_lamports" \
        "$stake_lamports"
    fi

    cat <<EOF
======================[ $node_type configuration ]======================
identity pubkey: $identity_pubkey
vote pubkey: $vote_pubkey
storage pubkey: $storage_pubkey
ledger: $ledger_config_dir
accounts: $accounts_config_dir
snapshots: $snapshot_config_dir
========================================================================
EOF
  fi

  echo "$PS4$program ${args[*]}"

  $program "${args[@]}" &
  pid=$!
  echo "pid: $pid"
  oom_score_adj "$pid" 1000

  if ((no_restart)); then
    wait "$pid"
    exit $?
  fi

  secs_to_next_genesis_poll=5
  secs_to_next_snapshot=30
  while true; do
    if ! kill -0 "$pid"; then
      wait "$pid" || true
      echo "############## $node_type exited, restarting ##############"
      break
    fi

    sleep 1

    if ((generate_snapshots && --secs_to_next_snapshot == 0)); then
      (
        SECONDS=
        new_state_dir="$SOLANA_RSYNC_CONFIG_DIR"/new_state
        new_state_archive="$SOLANA_RSYNC_CONFIG_DIR"/new_state.tgz
        (
          rm -rf "$new_state_dir" "$new_state_archive"
          mkdir -p "$new_state_dir"
          # When saving the state, its necessary to have the snapshots be saved first
          # followed by the accounts folder. This would avoid conditions where incomplete
          # accounts gets picked while its still in the process of being updated and are
          # not frozen yet.
          cp -a "$state_dir"/snapshots "$new_state_dir"
          cp -a "$state_dir"/accounts "$new_state_dir"
          cd "$new_state_dir"
          tar zcfS "$new_state_archive" ./*
        )
        ln -f "$new_state_archive" "$SOLANA_RSYNC_CONFIG_DIR"/state.tgz
        rm -rf "$new_state_dir" "$new_state_archive"
        ls -hl "$SOLANA_RSYNC_CONFIG_DIR"/state.tgz
        echo "Snapshot generated in $SECONDS seconds"
      ) || (
        echo "Error: failed to generate snapshot"
      )
      secs_to_next_snapshot=60
    fi

    if ((poll_for_new_genesis_block && --secs_to_next_genesis_poll == 0)); then
      echo "Polling for new genesis block..."
      if new_genesis_block; then
        echo "############## New genesis detected, restarting $node_type ##############"
        break
      fi
      secs_to_next_genesis_poll=60
    fi

  done

  kill_fullnode
  # give the cluster time to come back up
  (
    set -x
    sleep 60
  )
done
