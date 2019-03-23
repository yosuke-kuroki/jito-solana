# |source| this file
#
# Common utilities shared by other scripts in this directory
#
# The following directive disable complaints about unused variables in this
# file:
# shellcheck disable=2034
#

rsync=rsync
bootstrap_leader_logger="tee bootstrap-leader.log"
fullnode_logger="tee fullnode.log"
drone_logger="tee drone.log"

if [[ $(uname) != Linux ]]; then
  # Protect against unsupported configurations to prevent non-obvious errors
  # later. Arguably these should be fatal errors but for now prefer tolerance.
  if [[ -n $SOLANA_CUDA ]]; then
    echo "Warning: CUDA is not supported on $(uname)"
    SOLANA_CUDA=
  fi
fi


if [[ -n $USE_INSTALL || ! -f "$(dirname "${BASH_SOURCE[0]}")"/../Cargo.toml ]]; then
  solana_program() {
    declare program="$1"
    printf "solana-%s" "$program"
  }
else
  solana_program() {
    declare program="$1"
    declare features=""
    if [[ "$program" =~ ^(.*)-cuda$ ]]; then
      program=${BASH_REMATCH[1]}
      features="--features=cuda"
    fi

    if [[ -r "$(dirname "${BASH_SOURCE[0]}")"/../"$program"/Cargo.toml ]]; then
      maybe_package="--package solana-$program"
    fi
    if [[ -n $NDEBUG ]]; then
      maybe_release=--release
    fi
    printf "cargo run $maybe_release $maybe_package --bin solana-%s %s -- " "$program" "$features"
  }
  if [[ -n $SOLANA_CUDA ]]; then
    # shellcheck disable=2154 # 'here' is referenced but not assigned
    if [[ -z $here ]]; then
      echo "|here| is not defined"
      exit 1
    fi

    # Locate perf libs downloaded by |./fetch-perf-libs.sh|
    LD_LIBRARY_PATH=$(cd "$here" && dirname "$PWD"/target/perf-libs):$LD_LIBRARY_PATH
    export LD_LIBRARY_PATH
  fi
fi

solana_bench_tps=$(solana_program bench-tps)
solana_wallet=$(solana_program wallet)
solana_drone=$(solana_program drone)
solana_fullnode=$(solana_program fullnode)
solana_fullnode_cuda=$(solana_program fullnode-cuda)
solana_genesis=$(solana_program genesis)
solana_keygen=$(solana_program keygen)
solana_ledger_tool=$(solana_program ledger-tool)

export RUST_LOG=${RUST_LOG:-solana=info} # if RUST_LOG is unset, default to info
export RUST_BACKTRACE=1

# shellcheck source=scripts/configure-metrics.sh
source "$(dirname "${BASH_SOURCE[0]}")"/../scripts/configure-metrics.sh

tune_system() {
  # Skip in CI
  [[ -z $CI ]] || return 0

  # shellcheck source=scripts/ulimit-n.sh
  source "$(dirname "${BASH_SOURCE[0]}")"/../scripts/ulimit-n.sh

  # Reference: https://medium.com/@CameronSparr/increase-os-udp-buffers-to-improve-performance-51d167bb1360
  if [[ $(uname) = Linux ]]; then
    (
      set -x +e
      # test the existence of the sysctls before trying to set them
      # go ahead and return true and don't exit if these calls fail
      sysctl net.core.rmem_max 2>/dev/null 1>/dev/null &&
          sudo sysctl -w net.core.rmem_max=1610612736 1>/dev/null 2>/dev/null

      sysctl net.core.rmem_default 2>/dev/null 1>/dev/null &&
          sudo sysctl -w net.core.rmem_default=1610612736 1>/dev/null 2>/dev/null

      sysctl net.core.wmem_max 2>/dev/null 1>/dev/null &&
          sudo sysctl -w net.core.wmem_max=1610612736 1>/dev/null 2>/dev/null

      sysctl net.core.wmem_default 2>/dev/null 1>/dev/null &&
          sudo sysctl -w net.core.wmem_default=1610612736 1>/dev/null 2>/dev/null
    ) || true
  fi

  if [[ $(uname) = Darwin ]]; then
    (
      if [[ $(sysctl net.inet.udp.maxdgram | cut -d\  -f2) != 65535 ]]; then
        echo "Adjusting maxdgram to allow for large UDP packets, see BLOB_SIZE in src/packet.rs:"
        set -x
        sudo sysctl net.inet.udp.maxdgram=65535
      fi
    )

  fi
}

airdrop() {
  declare keypair_file=$1
  declare host=$2
  declare amount=$3

  declare address
  address=$($solana_wallet --keypair "$keypair_file" address)

  # TODO: Until https://github.com/solana-labs/solana/issues/2355 is resolved
  # a fullnode needs N lamports as its vote account gets re-created on every
  # node restart, costing it lamports
  declare retries=5

  while ! $solana_wallet --keypair "$keypair_file" --host "$host" airdrop "$amount"; do

    # TODO: Consider moving this retry logic into `solana-wallet airdrop`
    #   itself, currently it does not retry on "Connection refused" errors.
    ((retries--))
    if [[ $retries -le 0 ]]; then
        echo "Airdrop to $address failed."
        return 1
    fi
    echo "Airdrop to $address failed. Remaining retries: $retries"
    sleep 1
  done

  return 0
}

setup_fullnode_staking() {
  declare drone_address=$1
  declare fullnode_id_path=$2
  declare staker_id_path=$3

  declare fullnode_id
  fullnode_id=$($solana_wallet --keypair "$fullnode_id_path" address)

  declare staker_id
  staker_id=$($solana_wallet --keypair "$staker_id_path" address)

  if [[ -f "$staker_id_path".configured ]]; then
    echo "Staking account has already been configured"
    return 0
  fi

  # A fullnode requires 43 lamports to function:
  # - one lamport to keep the node identity public key valid. TODO: really??
  # - 42 more for the staker account we fund
  airdrop "$fullnode_id_path" "$drone_address" 43 || return $?

  # A little wrong, fund the staking account from the
  #  to the node.  Maybe next time consider doing this the opposite
  #  way or use an ephemeral account
  $solana_wallet --keypair "$fullnode_id_path" --host "$drone_address" \
               create-staking-account "$staker_id" 42 || return $?

  # as the staker, set the node as the delegate and the staker as
  #  the vote-signer
  $solana_wallet --keypair "$staker_id_path" --host "$drone_address" \
                 configure-staking-account \
                 --delegate-account "$fullnode_id" \
                 --authorize-voter "$staker_id"  || return $?


  touch "$staker_id_path".configured
  return 0
}

fullnode_usage() {
  if [[ -n $1 ]]; then
    echo "$*"
    echo
  fi
  cat <<EOF
usage: $0 [-x] [--blockstream PATH] [--init-complete-file FILE] [--only-bootstrap-stake] [--no-voting] [--rpc-port port] [rsync network path to bootstrap leader configuration] [network entry point]

Start a full node on the specified network

  -x                        - start a new, dynamically-configured full node. Does not apply to the bootstrap leader
  -X [label]                - start or restart a dynamically-configured full node with
                              the specified label. Does not apply to the bootstrap leader
  --blockstream PATH        - open blockstream at this unix domain socket location
  --init-complete-file FILE - create this file, if it doesn't already exist, once node initialization is complete
  --only-bootstrap-stake    - Only stake the bootstrap leader, effectively disabling leader rotation
  --public-address          - advertise public machine address in gossip.  By default the local machine address is advertised
  --no-voting               - start node without vote signer
  --rpc-port port           - custom RPC port for this node

EOF
  exit 1
}

# The directory on the bootstrap leader that is rsynced by other full nodes as
# they boot (TODO: Eventually this should go away)
SOLANA_RSYNC_CONFIG_DIR=$PWD/config

# Configuration that remains local
SOLANA_CONFIG_DIR=$PWD/config-local
