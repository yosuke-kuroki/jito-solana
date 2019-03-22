#!/usr/bin/env bash
#
# Start a full node
#
here=$(dirname "$0")
# shellcheck source=multinode-demo/common.sh
source "$here"/common.sh

# shellcheck source=scripts/oom-score-adj.sh
source "$here"/../scripts/oom-score-adj.sh

if [[ $1 = -h ]]; then
  fullnode_usage "$@"
fi

gossip_port=9000
extra_fullnode_args=()
self_setup=0
setup_stakes=1
poll_for_new_genesis_block=0

while [[ ${1:0:1} = - ]]; do
  if [[ $1 = -X ]]; then
    self_setup=1
    self_setup_label=$2
    shift 2
  elif [[ $1 = -x ]]; then
    self_setup=1
    self_setup_label=$$
    shift
  elif [[ $1 = --poll-for-new-genesis-block ]]; then
    poll_for_new_genesis_block=1
    shift
  elif [[ $1 = --blockstream ]]; then
    extra_fullnode_args+=("$1" "$2")
    shift 2
  elif [[ $1 = --enable-rpc-exit ]]; then
    extra_fullnode_args+=("$1")
    shift
  elif [[ $1 = --init-complete-file ]]; then
    extra_fullnode_args+=("$1" "$2")
    shift 2
  elif [[ $1 = --only-bootstrap-stake ]]; then
    setup_stakes=0
    shift
  elif [[ $1 = --public-address ]]; then
    extra_fullnode_args+=("$1")
    shift
  elif [[ $1 = --no-voting ]]; then
    extra_fullnode_args+=("$1")
    shift
  elif [[ $1 = --gossip-port ]]; then
    gossip_port=$2
    shift 2
  elif [[ $1 = --rpc-port ]]; then
    extra_fullnode_args+=("$1" "$2")
    shift 2
  else
    echo "Unknown argument: $1"
    exit 1
  fi
done

if [[ -n $3 ]]; then
  fullnode_usage "$@"
fi

find_leader() {
  declare leader leader_address
  declare shift=0

  if [[ -z $1 ]]; then
    leader=$PWD                   # Default to local tree for rsync
    leader_address=127.0.0.1:8001 # Default to local leader
  elif [[ -z $2 ]]; then
    leader=$1

    declare leader_ip
    if [[ $leader =~ ^[0-9]{1,3}.[0-9]{1,3}.[0-9]{1,3}.[0-9]{1,3}$ ]]; then
      leader_ip=$leader
    else
      leader_ip=$(dig +short "${leader%:*}" | head -n1)

      if [[ -z $leader_ip ]]; then
          usage "Error: unable to resolve IP address for $leader"
      fi
    fi

    leader_address=$leader_ip:8001
    shift=1
  else
    leader=$1
    leader_address=$2
    shift=2
  fi

  echo "$leader" "$leader_address" "$shift"
}

read -r leader leader_address shift < <(find_leader "${@:1:2}")
shift "$shift"

if [[ -n $SOLANA_CUDA ]]; then
  program=$solana_fullnode_cuda
else
  program=$solana_fullnode
fi

if ((!self_setup)); then
  [[ -f $SOLANA_CONFIG_DIR/fullnode-id.json ]] || {
    echo "$SOLANA_CONFIG_DIR/fullnode-id.json not found, create it by running:"
    echo
    echo "  ${here}/setup.sh"
    exit 1
  }
  fullnode_id_path=$SOLANA_CONFIG_DIR/fullnode-id.json
  fullnode_staker_id_path=$SOLANA_CONFIG_DIR/fullnode-staker-id.json
  ledger_config_dir=$SOLANA_CONFIG_DIR/fullnode-ledger
  accounts_config_dir=$SOLANA_CONFIG_DIR/fullnode-accounts
else
  mkdir -p "$SOLANA_CONFIG_DIR"
  fullnode_id_path=$SOLANA_CONFIG_DIR/fullnode-id-x$self_setup_label.json
  fullnode_staker_id_path=$SOLANA_CONFIG_DIR/fullnode-staker-id-x$self_setup_label.json

  [[ -f "$fullnode_id_path" ]] || $solana_keygen -o "$fullnode_id_path"
  [[ -f "$fullnode_staker_id_path" ]] || $solana_keygen -o "$fullnode_staker_id_path"

  echo "Finding a port.."
  # Find an available port in the range 9100-9899
  (( gossip_port = 9100 + ($$ % 800) ))
  while true; do
    (( gossip_port = gossip_port >= 9900 ? 9100 : ++gossip_port ))
    echo "Testing $gossip_port"
    if ! nc -w 10 -z 127.0.0.1 $gossip_port; then
      echo "Selected gossip_port $gossip_port"
      break;
    fi
    echo "Port $gossip_port is in use"
  done
  ledger_config_dir=$SOLANA_CONFIG_DIR/fullnode-ledger-x$self_setup_label
  accounts_config_dir=$SOLANA_CONFIG_DIR/fullnode-accounts-x$self_setup_label
fi

fullnode_id=$($solana_wallet --keypair "$fullnode_id_path" address)
fullnode_staker_id=$($solana_wallet --keypair "$fullnode_staker_id_path" address)


[[ -r $fullnode_id_path ]] || {
  echo "$fullnode_id_path does not exist"
  exit 1
}

tune_system

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


rsync_leader_url=$(rsync_url "$leader")
set -e

secs_to_next_genesis_poll=0
PS4="$(basename "$0"): "
while true; do
  set -x
  if [[ ! -d "$SOLANA_RSYNC_CONFIG_DIR"/ledger ]]; then
    $rsync -vPr "$rsync_leader_url"/config/ledger "$SOLANA_RSYNC_CONFIG_DIR"
  fi

  if [[ ! -d "$ledger_config_dir" ]]; then
    cp -a "$SOLANA_RSYNC_CONFIG_DIR"/ledger/ "$ledger_config_dir"
    $solana_ledger_tool --ledger "$ledger_config_dir" verify
  fi

  trap 'kill "$pid" && wait "$pid"' INT TERM ERR
  $program \
    --gossip-port "$gossip_port" \
    --identity "$fullnode_id_path" \
    --voting-keypair "$fullnode_staker_id_path" \
    --staking-account "$fullnode_staker_id" \
    --network "$leader_address" \
    --ledger "$ledger_config_dir" \
    --accounts "$accounts_config_dir" \
    --rpc-drone-address "${leader_address%:*}:9900" \
    "${extra_fullnode_args[@]}" \
    > >($fullnode_logger) 2>&1 &
  pid=$!
  oom_score_adj "$pid" 1000

  if ((setup_stakes)); then
    setup_fullnode_staking "${leader_address%:*}" "$fullnode_id_path" "$fullnode_staker_id_path"
  fi
  set +x

  while true; do
    if ! kill -0 "$pid"; then
      wait "$pid"
      exit 0
    fi
    sleep 1

    if ((poll_for_new_genesis_block)); then
      if ((!secs_to_next_genesis_poll)); then
        secs_to_next_genesis_poll=60

        $rsync -r "$rsync_leader_url"/config/ledger "$SOLANA_RSYNC_CONFIG_DIR" || true
        if [[ -n $(diff "$SOLANA_RSYNC_CONFIG_DIR"/ledger/genesis.json "$ledger_config_dir"/genesis.json 2>&1) ]]; then
          echo "############## New genesis detected, restarting fullnode ##############"
          rm -rf "$ledger_config_dir"
          kill "$pid" || true
          wait "$pid" || true
          break
        fi
      fi
      ((secs_to_next_genesis_poll--))
    fi
  done
done
