#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"/../..

set -x
deployMethod="$1"
nodeType="$2"
entrypointIp="$3"
numNodes="$4"
if [[ -n $5 ]]; then
  export RUST_LOG="$5"
fi
skipSetup="$6"
failOnValidatorBootupFailure="$7"
externalPrimordialAccountsFile="$8"
maybeDisableAirdrops="$9"
internalNodesStakeLamports="${10}"
internalNodesLamports="${11}"
nodeIndex="${12}"
numBenchTpsClients="${13}"
benchTpsExtraArgs="${14}"
numBenchExchangeClients="${15}"
benchExchangeExtraArgs="${16}"
genesisOptions="${17}"
extraNodeArgs="${18}"
set +x

# Use a very large stake (relative to the default multinode-demo/ stake of 42)
# for the testnet validators setup by net/.  This make it less likely that
# low-staked ephemeral validator a random user may attach to testnet will cause
# trouble
#
# Ref: https://github.com/solana-labs/solana/issues/3798
stake=${internalNodesStakeLamports:=424243}

missing() {
  echo "Error: $1 not specified"
  exit 1
}

[[ -n $deployMethod ]]  || missing deployMethod
[[ -n $nodeType ]]      || missing nodeType
[[ -n $entrypointIp ]]  || missing entrypointIp
[[ -n $numNodes ]]      || missing numNodes
[[ -n $skipSetup ]]     || missing skipSetup
[[ -n $failOnValidatorBootupFailure ]] || missing failOnValidatorBootupFailure

airdropsEnabled=true
if [[ -n $maybeDisableAirdrops ]]; then
  airdropsEnabled=false
fi
cat > deployConfig <<EOF
deployMethod="$deployMethod"
entrypointIp="$entrypointIp"
numNodes="$numNodes"
failOnValidatorBootupFailure=$failOnValidatorBootupFailure
genesisOptions="$genesisOptions"
airdropsEnabled=$airdropsEnabled
EOF

source scripts/oom-score-adj.sh
source net/common.sh
loadConfigFile

initCompleteFile=init-complete-node.log
waitForNodeToInit() {
  echo "--- waiting for node to boot up"
  SECONDS=
  while [[ ! -r $initCompleteFile ]]; do
    if [[ $SECONDS -ge 720 ]]; then
      echo "^^^ +++"
      echo "Error: $initCompleteFile not found in $SECONDS seconds"
      exit 1
    fi
    echo "Waiting for $initCompleteFile ($SECONDS)..."
    sleep 5
  done
  echo "Node booted up"
}

case $deployMethod in
local|tar|skip)
  PATH="$HOME"/.cargo/bin:"$PATH"
  export USE_INSTALL=1

  ./fetch-perf-libs.sh
  # shellcheck source=/dev/null
  source ./target/perf-libs/env.sh
  SUDO_OK=1 source scripts/tune-system.sh

  (
    sudo SOLANA_METRICS_CONFIG="$SOLANA_METRICS_CONFIG" scripts/oom-monitor.sh
  ) > oom-monitor.log 2>&1 &
  echo $! > oom-monitor.pid
  scripts/fd-monitor.sh > fd-monitor.log 2>&1 &
  echo $! > fd-monitor.pid
  scripts/net-stats.sh  > net-stats.log 2>&1 &
  echo $! > net-stats.pid

  case $nodeType in
  bootstrap-leader)
    if [[ -e /dev/nvidia0 && -x ~/.cargo/bin/solana-validator-cuda ]]; then
      echo Selecting solana-validator-cuda
      export SOLANA_CUDA=1
    fi
    set -x
    if [[ $skipSetup != true ]]; then
      rm -rf ./solana-node-keys
      rm -rf ./solana-node-balances
      mkdir ./solana-node-balances
      if [[ -n $internalNodesLamports ]]; then
        echo "---" >> ./solana-node-balances/fullnode-balances.yml
        for i in $(seq 0 "$numNodes"); do
          solana-keygen new -o ./solana-node-keys/"$i"
          pubkey="$(solana-keygen pubkey ./solana-node-keys/"$i")"
          cat >> ./solana-node-balances/fullnode-balances.yml <<EOF
$pubkey:
  balance: $internalNodesLamports
  owner: 11111111111111111111111111111111
  data:
  executable: false
EOF
        done
      fi

      lamports_per_signature="42"
      # shellcheck disable=SC2206 # Do not want to quote $genesisOptions
      genesis_args=($genesisOptions)
      for i in "${!genesis_args[@]}"; do
        if [[ "${genesis_args[$i]}" = --target-lamports-per-signature ]]; then
          lamports_per_signature="${genesis_args[$((i+1))]}"
          break
        fi
      done

      rm -rf ./solana-client-accounts
      mkdir ./solana-client-accounts
      for i in $(seq 0 $((numBenchTpsClients-1))); do
        # shellcheck disable=SC2086 # Do not want to quote $benchTpsExtraArgs
        solana-bench-tps --write-client-keys ./solana-client-accounts/bench-tps"$i".yml \
          --target-lamports-per-signature "$lamports_per_signature" $benchTpsExtraArgs
        # Skip first line, as it contains header
        tail -n +2 -q ./solana-client-accounts/bench-tps"$i".yml >> ./solana-client-accounts/client-accounts.yml
        echo "" >> ./solana-client-accounts/client-accounts.yml
      done
      for i in $(seq 0 $((numBenchExchangeClients-1))); do
        # shellcheck disable=SC2086 # Do not want to quote $benchExchangeExtraArgs
        solana-bench-exchange --batch-size 1000 --fund-amount 20000 \
          --write-client-keys ./solana-client-accounts/bench-exchange"$i".yml $benchExchangeExtraArgs
        tail -n +2 -q ./solana-client-accounts/bench-exchange"$i".yml >> ./solana-client-accounts/client-accounts.yml
        echo "" >> ./solana-client-accounts/client-accounts.yml
      done
      [[ -z $externalPrimordialAccountsFile ]] || cat "$externalPrimordialAccountsFile" >> ./solana-node-balances/fullnode-balances.yml
      if [ -f ./solana-node-balances/fullnode-balances.yml ]; then
        genesisOptions+=" --primordial-accounts-file ./solana-node-balances/fullnode-balances.yml"
      fi
      if [ -f ./solana-client-accounts/client-accounts.yml ]; then
        genesisOptions+=" --primordial-keypairs-file ./solana-client-accounts/client-accounts.yml"
      fi

      args=(
        --bootstrap-leader-stake-lamports "$stake"
      )
      if [[ -n $internalNodesLamports ]]; then
        args+=(--bootstrap-leader-lamports "$internalNodesLamports")
      fi
      # shellcheck disable=SC2206 # Do not want to quote $genesisOptions
      args+=($genesisOptions)
      ./multinode-demo/setup.sh "${args[@]}"
    fi
    args=(
      --gossip-port "$entrypointIp":8001
      --init-complete-file "$initCompleteFile"
    )

    if [[ $airdropsEnabled = true ]]; then
      ./multinode-demo/drone.sh > drone.log 2>&1 &
    fi
    # shellcheck disable=SC2206 # Don't want to double quote $extraNodeArgs
    args+=($extraNodeArgs)
    nohup ./multinode-demo/bootstrap-leader.sh "${args[@]}" > fullnode.log 2>&1 &
    pid=$!
    oom_score_adj "$pid" 1000
    waitForNodeToInit

    solana --url http://"$entrypointIp":8899 -k ~/solana/fullnode-identity.json \
      validator-info publish "$(hostname)" -n team/solana --force || true
    ;;
  validator|blockstreamer)
    if [[ $deployMethod != skip ]]; then
      net/scripts/rsync-retry.sh -vPrc "$entrypointIp":~/.cargo/bin/ ~/.cargo/bin/
    fi
    if [[ $skipSetup != true ]]; then
      rm -f ~/solana/fullnode-identity.json
      [[ -z $internalNodesLamports ]] || net/scripts/rsync-retry.sh -vPrc \
      "$entrypointIp":~/solana/solana-node-keys/"$nodeIndex" ~/solana/fullnode-identity.json
    fi

    if [[ -e /dev/nvidia0 && -x ~/.cargo/bin/solana-validator-cuda ]]; then
      echo Selecting solana-validator-cuda
      export SOLANA_CUDA=1
    fi

    args=(
      --entrypoint "$entrypointIp:8001"
      --gossip-port 8001
      --rpc-port 8899
    )
    if [[ $nodeType = blockstreamer ]]; then
      args+=(
        --blockstream /tmp/solana-blockstream.sock
        --no-voting
      )
    else
      args+=(--enable-rpc-exit)
      if [[ -n $internalNodesLamports ]]; then
        args+=(--node-lamports "$internalNodesLamports")
      fi
    fi

    if [[ ! -f ~/solana/fullnode-identity.json ]]; then
      solana-keygen new -o ~/solana/fullnode-identity.json
    fi
    args+=(--identity ~/solana/fullnode-identity.json)

    if [[ $airdropsEnabled != true ]]; then
      args+=(--no-airdrop)
    fi

    set -x
    if [[ $skipSetup != true ]]; then
      ./multinode-demo/clear-config.sh
    fi

    if [[ $nodeType = blockstreamer ]]; then
      # Sneak the mint-keypair.json from the bootstrap leader and run another drone
      # with it on the blockstreamer node.  Typically the blockstreamer node has
      # a static IP/DNS name for hosting the blockexplorer web app, and is
      # a location that somebody would expect to be able to airdrop from
      scp "$entrypointIp":~/solana/config/mint-keypair.json config/
      if [[ $airdropsEnabled = true ]]; then
        ./multinode-demo/drone.sh > drone.log 2>&1 &
      fi

      # Grab the TLS cert generated by /certbot-restore.sh
      if [[ -f /.cert.pem ]]; then
        sudo install -o $UID -m 400 /.cert.pem /.key.pem .
        ls -l .cert.pem .key.pem
      fi

      export BLOCKEXPLORER_GEOIP_WHITELIST=$PWD/net/config/geoip.yml
      npm install @solana/blockexplorer@1
      npx solana-blockexplorer > blockexplorer.log 2>&1 &

      # Confirm the blockexplorer is accessible
      curl --head --retry 3 --retry-connrefused http://localhost:5000/

      # Redirect port 80 to port 5000
      sudo iptables -A INPUT -p tcp --dport 80 -j ACCEPT
      sudo iptables -A INPUT -p tcp --dport 5000 -j ACCEPT
      sudo iptables -A PREROUTING -t nat -p tcp --dport 80 -j REDIRECT --to-port 5000

      # Confirm the blockexplorer is now globally accessible
      curl --head "$(curl ifconfig.io)"
    fi

    args+=(--init-complete-file "$initCompleteFile")
    # shellcheck disable=SC2206 # Don't want to double quote $extraNodeArgs
    args+=($extraNodeArgs)
    nohup ./multinode-demo/validator.sh "${args[@]}" > fullnode.log 2>&1 &
    pid=$!
    oom_score_adj "$pid" 1000
    waitForNodeToInit

    if [[ $skipSetup != true && $nodeType != blockstreamer ]]; then
      args=(
        --url http://"$entrypointIp":8899
        --force
        "$stake"
      )
      if [[ $airdropsEnabled != true ]]; then
        args+=(--no-airdrop)
      fi
      if [[ -f ~/solana/fullnode-identity.json ]]; then
        args+=(--keypair ~/solana/fullnode-identity.json)
      fi

      ./multinode-demo/delegate-stake.sh "${args[@]}"
    fi

    solana --url http://"$entrypointIp":8899 -k ~/solana/fullnode-identity.json \
      validator-info publish "$(hostname)" -n team/solana --force || true
    ;;
  replicator)
    if [[ $deployMethod != skip ]]; then
      net/scripts/rsync-retry.sh -vPrc "$entrypointIp":~/.cargo/bin/ ~/.cargo/bin/
    fi

    args=(
      --entrypoint "$entrypointIp:8001"
    )

    if [[ $airdropsEnabled != true ]]; then
      echo "TODO: replicators not supported without airdrops"
      # TODO: need to provide the `--identity` argument to an existing system
      #       account with lamports in it
      exit 1
    fi

    nohup ./multinode-demo/replicator.sh "${args[@]}" > fullnode.log 2>&1 &
    pid=$!
    oom_score_adj "$pid" 1000
    sleep 1
    ;;
  *)
    echo "Error: unknown node type: $nodeType"
    exit 1
    ;;
  esac
  disown
  ;;
*)
  echo "Unknown deployment method: $deployMethod"
  exit 1
esac
