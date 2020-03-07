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
gpuMode="${19:-auto}"
GEOLOCATION_API_KEY="${20}"
set +x

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

source net/common.sh
source multinode-demo/common.sh
loadConfigFile

initCompleteFile=init-complete-node.log

cat > ~/solana/on-reboot <<EOF
#!/usr/bin/env bash
cd ~/solana
source scripts/oom-score-adj.sh

now=\$(date -u +"%Y-%m-%dT%H:%M:%SZ")
ln -sfT validator.log.\$now validator.log
EOF
chmod +x ~/solana/on-reboot

GPU_CUDA_OK=false
GPU_FAIL_IF_NONE=false
case "$gpuMode" in
  on) # GPU *required*, any vendor
    GPU_CUDA_OK=true
    GPU_FAIL_IF_NONE=true
    ;;
  off) # CPU-only
    ;;
  auto) # Use GPU if installed, any vendor
    GPU_CUDA_OK=true
    ;;
  cuda) # GPU *required*, CUDA-only
    GPU_CUDA_OK=true
    GPU_FAIL_IF_NONE=true
    ;;
  *)
    echo "Unexpected gpuMode: \"$gpuMode\""
    exit 1
    ;;
esac

waitForNodeToInit() {
  hostname=$(hostname)
  echo "--- waiting for $hostname to boot up"
  SECONDS=
  while [[ ! -r $initCompleteFile ]]; do
    if [[ $SECONDS -ge 240 ]]; then
      echo "^^^ +++"
      echo "Error: $initCompleteFile not found in $SECONDS seconds"
      exit 1
    fi
    echo "Waiting for $initCompleteFile ($SECONDS) on $hostname..."
    sleep 5
  done
  echo "$hostname booted up"
}

case $deployMethod in
local|tar|skip)
  PATH="$HOME"/.cargo/bin:"$PATH"
  export USE_INSTALL=1

  ./fetch-perf-libs.sh

cat >> ~/solana/on-reboot <<EOF
  PATH="$HOME"/.cargo/bin:"$PATH"
  export USE_INSTALL=1

  sudo RUST_LOG=info ~solana/.cargo/bin/solana-sys-tuner --user $(whoami) > sys-tuner.log 2>&1 &
  echo \$! > sys-tuner.pid

  (
    sudo SOLANA_METRICS_CONFIG="$SOLANA_METRICS_CONFIG" scripts/oom-monitor.sh
  ) > oom-monitor.log 2>&1 &
  echo \$! > oom-monitor.pid
  scripts/fd-monitor.sh > fd-monitor.log 2>&1 &
  echo \$! > fd-monitor.pid
  scripts/net-stats.sh  > net-stats.log 2>&1 &
  echo \$! > net-stats.pid
  scripts/iftop.sh  > iftop.log 2>&1 &
  echo \$! > iftop.pid
  scripts/system-stats.sh  > system-stats.log 2>&1 &
  echo \$! > system-stats.pid

  if ${GPU_CUDA_OK} && [[ -e /dev/nvidia0 ]]; then
    echo Selecting solana-validator-cuda
    export SOLANA_CUDA=1
  elif ${GPU_FAIL_IF_NONE} ; then
    echo "Expected GPU, found none!"
    export SOLANA_GPU_MISSING=1
  fi
EOF

  case $nodeType in
  bootstrap-validator)
    set -x
    if [[ $skipSetup != true ]]; then
      clear_config_dir "$SOLANA_CONFIG_DIR"

      if [[ -n $internalNodesLamports ]]; then
        echo "---" >> config/validator-balances.yml
      fi

      setupValidatorKeypair() {
        declare name=$1
        if [[ -f net/keypairs/"$name".json ]]; then
          cp net/keypairs/"$name".json config/"$name".json
        else
          solana-keygen new --no-passphrase -so config/"$name".json
        fi
        if [[ -n $internalNodesLamports ]]; then
          declare pubkey
          pubkey="$(solana-keygen pubkey config/"$name".json)"
          cat >> config/validator-balances.yml <<EOF
$pubkey:
  balance: $internalNodesLamports
  owner: 11111111111111111111111111111111
  data:
  executable: false
EOF
        fi
      }
      for i in $(seq 1 "$numNodes"); do
        setupValidatorKeypair validator-identity-"$i"
      done
      setupValidatorKeypair blockstreamer-identity

      lamports_per_signature="42"
      # shellcheck disable=SC2206 # Do not want to quote $genesisOptions
      genesis_args=($genesisOptions)
      for i in "${!genesis_args[@]}"; do
        if [[ "${genesis_args[$i]}" = --target-lamports-per-signature ]]; then
          lamports_per_signature="${genesis_args[$((i+1))]}"
          break
        fi
      done

      for i in $(seq 0 $((numBenchTpsClients-1))); do
        # shellcheck disable=SC2086 # Do not want to quote $benchTpsExtraArgs
        solana-bench-tps --write-client-keys config/bench-tps"$i".yml \
          --target-lamports-per-signature "$lamports_per_signature" $benchTpsExtraArgs
        # Skip first line, as it contains header
        tail -n +2 -q config/bench-tps"$i".yml >> config/client-accounts.yml
        echo "" >> config/client-accounts.yml
      done
      for i in $(seq 0 $((numBenchExchangeClients-1))); do
        # shellcheck disable=SC2086 # Do not want to quote $benchExchangeExtraArgs
        solana-bench-exchange --batch-size 1000 --fund-amount 20000 \
          --write-client-keys config/bench-exchange"$i".yml $benchExchangeExtraArgs
        tail -n +2 -q config/bench-exchange"$i".yml >> config/client-accounts.yml
        echo "" >> config/client-accounts.yml
      done
      if [[ -f $externalPrimordialAccountsFile ]]; then
        cat "$externalPrimordialAccountsFile" >> config/validator-balances.yml
      fi
      if [[ -f config/validator-balances.yml ]]; then
        genesisOptions+=" --primordial-accounts-file config/validator-balances.yml"
      fi
      if [[ -f config/client-accounts.yml ]]; then
        genesisOptions+=" --primordial-accounts-file config/client-accounts.yml"
      fi

      if [[ -n $internalNodesStakeLamports ]]; then
        args+=(--bootstrap-validator-stake-lamports "$internalNodesStakeLamports")
      fi
      if [[ -n $internalNodesLamports ]]; then
        args+=(--bootstrap-validator-lamports "$internalNodesLamports")
      fi
      # shellcheck disable=SC2206 # Do not want to quote $genesisOptions
      args+=($genesisOptions)

      if [[ -f net/keypairs/faucet.json ]]; then
        export FAUCET_KEYPAIR=net/keypairs/faucet.json
      fi
      if [[ -f net/keypairs/bootstrap-validator-identity.json ]]; then
        export BOOTSTRAP_VALIDATOR_IDENTITY_KEYPAIR=net/keypairs/bootstrap-validator-identity.json
      fi
      multinode-demo/setup.sh "${args[@]}"

      solana-ledger-tool -l config/bootstrap-validator shred-version | tee config/shred-version
    fi
    args=(
      --gossip-host "$entrypointIp"
      --gossip-port 8001
      --init-complete-file "$initCompleteFile"
    )

    if [[ $airdropsEnabled = true ]]; then
cat >> ~/solana/on-reboot <<EOF
      ./multinode-demo/faucet.sh > faucet.log 2>&1 &
EOF
    fi
    # shellcheck disable=SC2206 # Don't want to double quote $extraNodeArgs
    args+=($extraNodeArgs)

cat >> ~/solana/on-reboot <<EOF
    nohup ./multinode-demo/bootstrap-validator.sh ${args[@]} > validator.log.\$now 2>&1 &
    pid=\$!
    oom_score_adj "\$pid" 1000
    disown
EOF
    ~/solana/on-reboot
    waitForNodeToInit

    if [[ $skipSetup != true ]]; then
      solana --url http://"$entrypointIp":8899 \
        --keypair ~/solana/config/bootstrap-validator/identity-keypair.json \
        validator-info publish "$(hostname)" -n team/solana --force || true
    fi
    ;;
  validator|blockstreamer)
    if [[ $deployMethod != skip ]]; then
      net/scripts/rsync-retry.sh -vPrc "$entrypointIp":~/.cargo/bin/ ~/.cargo/bin/
      net/scripts/rsync-retry.sh -vPrc "$entrypointIp":~/version.yml ~/version.yml
    fi
    if [[ $skipSetup != true ]]; then
      clear_config_dir "$SOLANA_CONFIG_DIR"

      if [[ $nodeType = blockstreamer ]]; then
        net/scripts/rsync-retry.sh -vPrc \
          "$entrypointIp":~/solana/config/blockstreamer-identity.json config/validator-identity.json
      else
        net/scripts/rsync-retry.sh -vPrc \
          "$entrypointIp":~/solana/config/validator-identity-"$nodeIndex".json config/validator-identity.json
      fi
      net/scripts/rsync-retry.sh -vPrc \
        "$entrypointIp":~/solana/config/shred-version config/shred-version
    fi

    args=(
      --entrypoint "$entrypointIp:8001"
      --gossip-port 8001
      --rpc-port 8899
      --expected-shred-version "$(cat config/shred-version)"
    )
    if [[ $nodeType = blockstreamer ]]; then
      args+=(
        --blockstream /tmp/solana-blockstream.sock
        --no-voting
        --dev-no-sigverify
        --enable-rpc-get-confirmed-block
      )
    else
      if [[ -n $internalNodesLamports ]]; then
        args+=(--node-lamports "$internalNodesLamports")
      fi
    fi

    if [[ ! -f config/validator-identity.json ]]; then
      solana-keygen new --no-passphrase -so config/validator-identity.json
    fi
    args+=(--identity-keypair config/validator-identity.json)

    if [[ $airdropsEnabled != true ]]; then
      args+=(--no-airdrop)
    fi

    set -x
    # Add the faucet keypair to validators for convenient access from tools
    # like bench-tps and add to blocktreamers to run a faucet
    scp "$entrypointIp":~/solana/config/faucet-keypair.json config/
    if [[ $nodeType = blockstreamer ]]; then
      # Run another faucet with the same keypair on the blockstreamer node.
      # Typically the blockstreamer node has a static IP/DNS name for hosting
      # the blockexplorer web app, and is a location that somebody would expect
      # to be able to airdrop from
      scp "$entrypointIp":~/solana/config/faucet-keypair.json config/
      if [[ $airdropsEnabled = true ]]; then
cat >> ~/solana/on-reboot <<EOF
        multinode-demo/faucet.sh > faucet.log 2>&1 &
EOF
      fi

      # Grab the TLS cert generated by /certbot-restore.sh
      if [[ -f /.cert.pem ]]; then
        sudo install -o $UID -m 400 /.cert.pem /.key.pem .
        ls -l .cert.pem .key.pem
      fi

      cat > ~/solana/restart-explorer <<EOF
#!/bin/bash -ex
      cd ~/solana

      export GEOLOCATION_API_KEY=$GEOLOCATION_API_KEY

      if [[ -f blockexplorer.pid ]]; then
        pgid=\$(ps opgid= \$(cat blockexplorer.pid) | tr -d '[:space:]')
        if [[ -n \$pgid ]]; then
          kill -- -\$pgid
        fi
      fi
      killall node || true
      npm install @solana/blockexplorer@1
      export BLOCKEXPLORER_GEOIP_WHITELIST=$PWD/net/config/geoip.yml
      npx solana-blockexplorer > blockexplorer.log 2>&1 &
      echo \$! > blockexplorer.pid

      # Redirect port 80 to port 5000
      sudo iptables -A INPUT -p tcp --dport 80 -j ACCEPT
      sudo iptables -A INPUT -p tcp --dport 5000 -j ACCEPT
      sudo iptables -A PREROUTING -t nat -p tcp --dport 80 -j REDIRECT --to-port 5000

      # Confirm the explorer is accessible
      curl --head --retry 3 --retry-connrefused http://localhost:5000/

      # Confirm the explorer is now globally accessible
      curl --head "\$(curl ifconfig.io)"
EOF
      chmod +x ~/solana/restart-explorer

cat >> ~/solana/on-reboot <<EOF
      ~/solana/restart-explorer
EOF
    fi

    args+=(--init-complete-file "$initCompleteFile")
    # shellcheck disable=SC2206 # Don't want to double quote $extraNodeArgs
    args+=($extraNodeArgs)
cat >> ~/solana/on-reboot <<EOF
    nohup multinode-demo/validator.sh ${args[@]} > validator.log.\$now 2>&1 &
    pid=\$!
    oom_score_adj "\$pid" 1000
    disown
EOF
    ~/solana/on-reboot
    waitForNodeToInit

    if [[ $skipSetup != true && $nodeType != blockstreamer ]]; then
      # Wait for the validator to catch up to the bootstrap validator before
      # delegating stake to it
      solana --url http://"$entrypointIp":8899 catchup config/validator-identity.json

      args=(
        --url http://"$entrypointIp":8899
      )
      if [[ $airdropsEnabled != true ]]; then
        args+=(--no-airdrop)
      fi
      if [[ -f config/validator-identity.json ]]; then
        args+=(--keypair config/validator-identity.json)
      fi

      multinode-demo/delegate-stake.sh "${args[@]}" "$internalNodesStakeLamports"
    fi

    if [[ $skipSetup != true ]]; then
      solana --url http://"$entrypointIp":8899 \
        --keypair config/validator-identity.json \
        validator-info publish "$(hostname)" -n team/solana --force || true
    fi
    ;;
  archiver)
    if [[ $deployMethod != skip ]]; then
      net/scripts/rsync-retry.sh -vPrc "$entrypointIp":~/.cargo/bin/ ~/.cargo/bin/
    fi

    args=(
      --entrypoint "$entrypointIp:8001"
    )

    if [[ $airdropsEnabled != true ]]; then
      # If this ever becomes a problem, we need to provide the `--identity-keypair`
      # argument to an existing system account with lamports in it
      echo "Error: archivers not supported without airdrops"
      exit 1
    fi

cat >> ~/solana/on-reboot <<EOF
    nohup multinode-demo/archiver.sh ${args[@]} > validator.log.\$now 2>&1 &
    pid=\$!
    oom_score_adj "\$pid" 1000
    disown
EOF
    ~/solana/on-reboot
    sleep 1
    ;;
  *)
    echo "Error: unknown node type: $nodeType"
    exit 1
    ;;
  esac
  ;;
*)
  echo "Unknown deployment method: $deployMethod"
  exit 1
esac
