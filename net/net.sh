#!/usr/bin/env bash
set -e

here=$(dirname "$0")
SOLANA_ROOT="$(cd "$here"/..; pwd)"

# shellcheck source=net/common.sh
source "$here"/common.sh

usage() {
  exitcode=0
  if [[ -n "$1" ]]; then
    exitcode=1
    echo "Error: $*"
  fi
  cat <<EOF
usage: $0 [start|stop|restart|sanity] [command-specific options]

Operate a configured testnet

 start    - Start the network
 sanity   - Sanity check the network
 stop     - Stop the network
 restart  - Shortcut for stop then start
 logs     - Fetch remote logs from each network node
 startnode- Start an individual node (previously stopped with stopNode)
 stopnode - Stop an individual node
 update   - Deploy a new software update to the cluster

 start-specific options:
   -T [tarFilename]                   - Deploy the specified release tarball
   -t edge|beta|stable|vX.Y.Z         - Deploy the latest tarball release for the
                                        specified release channel (edge|beta|stable) or release tag
                                        (vX.Y.Z)
   -r / --skip-setup                  - Reuse existing node/ledger configuration from a
                                        previous |start| (ie, don't run ./multinode-demo/setup.sh).
   -d / --debug                       - Build/deploy the testnet with debug binaries
   -D /path/to/programs               - Deploy custom programs from this location
   -c clientType=numClients=extraArgs - Number of clientTypes to start.  This options can be specified
                                        more than once.  Defaults to bench-tps for all clients if not
                                        specified.
                                        Valid client types are:
                                            bench-tps
                                            bench-exchange
                                        User can optionally provide extraArgs that are transparently
                                        supplied to the client program as command line parameters.
                                        For example,
                                            -c bench-tps=2="--tx_count 25000"
                                        This will start 2 bench-tps clients, and supply "--tx_count 25000"
                                        to the bench-tps client.
   -n NUM_FULL_NODES                  - Number of fullnodes to apply command to.
   --gpu-mode GPU_MODE                - Specify GPU mode to launch validators with (default: $gpuMode).
                                        MODE must be one of
                                          on - GPU *required*, any vendor *
                                          off - No GPU, CPU-only
                                          auto - Use GPU if available, any vendor *
                                          cuda - GPU *required*, Nvidia CUDA only
                                          *  Currently, Nvidia CUDA is the only supported GPU vendor
   --hashes-per-tick NUM_HASHES|sleep|auto
                                      - Override the default --hashes-per-tick for the cluster
   --no-airdrop
                                      - If set, disables airdrops.  Nodes must be funded in genesis block when airdrops are disabled.
   --lamports NUM_LAMPORTS_TO_MINT
                                      - Override the default 100000000000000 lamports minted in genesis
   --internal-nodes-stake-lamports NUM_LAMPORTS_PER_NODE
                                      - Amount to stake internal nodes.
   --internal-nodes-lamports NUM_LAMPORTS_PER_NODE
                                      - Amount to fund internal nodes in genesis block.
   --external-accounts-file FILE_PATH
                                      - A YML file with a list of account pubkeys and corresponding lamport balances
                                        in genesis block for external nodes
   --no-snapshot-fetch
                                      - If set, disables booting validators from a snapshot
   --skip-ledger-verify
                                      - If set, validators will skip verifying
                                        the ledger they already have saved to disk at
                                        boot (results in a much faster boot)
   --no-deploy
                                      - Don't deploy new software, use the
                                        existing deployment
   --no-build
                                      - Don't build new software, deploy the
                                        existing binaries

   --deploy-if-newer                  - Only deploy if newer software is
                                        available (requires -t or -T)

 sanity/start-specific options:
   -F                   - Discard validator nodes that didn't bootup successfully
   -o noValidatorSanity - Skip fullnode sanity
   -o noInstallCheck    - Skip solana-install sanity
   -o rejectExtraNodes  - Require the exact number of nodes

 stop-specific options:
   none

 logs-specific options:
   none

 update-specific options:
   --platform linux|osx|windows       - Deploy the tarball using 'solana-install deploy ...' for the
                                        given platform (multiple platforms may be specified)
                                        (-t option must be supplied as well)

 startnode/stopnode-specific options:
   -i [ip address]                    - IP Address of the node to start or stop

Note: if RUST_LOG is set in the environment it will be propogated into the
      network nodes.
EOF
  exit $exitcode
}

releaseChannel=
deployMethod=local
deployIfNewer=
sanityExtraArgs=
skipSetup=false
customPrograms=
updatePlatforms=
nodeAddress=
numBenchTpsClients=0
numBenchExchangeClients=0
benchTpsExtraArgs=
benchExchangeExtraArgs=
failOnValidatorBootupFailure=true
genesisOptions=
numFullnodesRequested=
externalPrimordialAccountsFile=
remoteExternalPrimordialAccountsFile=
internalNodesStakeLamports=
internalNodesLamports=
maybeNoSnapshot=""
maybeLimitLedgerSize=""
maybeSkipLedgerVerify=""
maybeDisableAirdrops=""
debugBuild=false
doBuild=true
gpuMode=auto

command=$1
[[ -n $command ]] || usage
shift

shortArgs=()
while [[ -n $1 ]]; do
  if [[ ${1:0:2} = -- ]]; then
    if [[ $1 = --hashes-per-tick ]]; then
      genesisOptions="$genesisOptions $1 $2"
      shift 2
    elif [[ $1 = --target-lamports-per-signature ]]; then
      genesisOptions="$genesisOptions $1 $2"
      shift 2
    elif [[ $1 = --lamports ]]; then
      genesisOptions="$genesisOptions $1 $2"
      shift 2
    elif [[ $1 = --no-snapshot-fetch ]]; then
      maybeNoSnapshot="$1"
      shift 1
    elif [[ $1 = --deploy-if-newer ]]; then
      deployIfNewer=1
      shift 1
    elif [[ $1 = --no-deploy ]]; then
      deployMethod=skip
      shift 1
    elif [[ $1 = --no-build ]]; then
      doBuild=false
      shift 1
    elif [[ $1 = --limit-ledger-size ]]; then
      maybeLimitLedgerSize="$1"
      shift 1
    elif [[ $1 = --skip-ledger-verify ]]; then
      maybeSkipLedgerVerify="$1"
      shift 1
    elif [[ $1 = --skip-setup ]]; then
      skipSetup=true
      shift 1
    elif [[ $1 = --platform ]]; then
      updatePlatforms="$updatePlatforms $2"
      shift 2
    elif [[ $1 = --internal-nodes-stake-lamports ]]; then
      internalNodesStakeLamports="$2"
      shift 2
    elif [[ $1 = --internal-nodes-lamports ]]; then
      internalNodesLamports="$2"
      shift 2
    elif [[ $1 = --external-accounts-file ]]; then
      externalPrimordialAccountsFile="$2"
      remoteExternalPrimordialAccountsFile=/tmp/external-primordial-accounts.yml
      shift 2
    elif [[ $1 = --no-airdrop ]]; then
      maybeDisableAirdrops="$1"
      shift 1
    elif [[ $1 = --debug ]]; then
      debugBuild=true
      shift 1
    elif [[ $1 = --gpu-mode ]]; then
      gpuMode=$2
      case "$gpuMode" in
        on|off|auto|cuda)
          ;;
        *)
          echo "Unexpected GPU mode: \"$gpuMode\""
          exit 1
          ;;
      esac
      shift 2
    else
      usage "Unknown long option: $1"
    fi
  else
    shortArgs+=("$1")
    shift
  fi
done

while getopts "h?T:t:o:f:rD:c:Fn:i:d" opt "${shortArgs[@]}"; do
  case $opt in
  h | \?)
    usage
    ;;
  T)
    tarballFilename=$OPTARG
    [[ -r $tarballFilename ]] || usage "File not readable: $tarballFilename"
    deployMethod=tar
    ;;
  t)
    case $OPTARG in
    edge|beta|stable|v*)
      releaseChannel=$OPTARG
      deployMethod=tar
      ;;
    *)
      usage "Invalid release channel: $OPTARG"
      ;;
    esac
    ;;
  n)
    numFullnodesRequested=$OPTARG
    ;;
  r)
    skipSetup=true
    ;;
  D)
    customPrograms=$OPTARG
    ;;
  o)
    case $OPTARG in
    noValidatorSanity|rejectExtraNodes|noInstallCheck)
      sanityExtraArgs="$sanityExtraArgs -o $OPTARG"
      ;;
    *)
      usage "Unknown option: $OPTARG"
      ;;
    esac
    ;;
  c)
    getClientTypeAndNum() {
      if ! [[ $OPTARG == *'='* ]]; then
        echo "Error: Expecting tuple \"clientType=numClientType=extraArgs\" but got \"$OPTARG\""
        exit 1
      fi
      local keyValue
      IFS='=' read -ra keyValue <<< "$OPTARG"
      local clientType=${keyValue[0]}
      local numClients=${keyValue[1]}
      local extraArgs=${keyValue[2]}
      re='^[0-9]+$'
      if ! [[ $numClients =~ $re ]] ; then
        echo "error: numClientType must be a number but got \"$numClients\""
        exit 1
      fi
      case $clientType in
        bench-tps)
          numBenchTpsClients=$numClients
          benchTpsExtraArgs=$extraArgs
        ;;
        bench-exchange)
          numBenchExchangeClients=$numClients
          benchExchangeExtraArgs=$extraArgs
        ;;
        *)
          echo "Unknown client type: $clientType"
          exit 1
          ;;
      esac
    }
    getClientTypeAndNum
    ;;
  F)
    failOnValidatorBootupFailure=false
    ;;
  i)
    nodeAddress=$OPTARG
    ;;
  d)
    debugBuild=true
    ;;
  *)
    usage "Error: unhandled option: $opt"
    ;;
  esac
done

loadConfigFile

if [[ -n $numFullnodesRequested ]]; then
  truncatedNodeList=( "${fullnodeIpList[@]:0:$numFullnodesRequested}" )
  unset fullnodeIpList
  fullnodeIpList=( "${truncatedNodeList[@]}" )
fi

numClients=${#clientIpList[@]}
numClientsRequested=$((numBenchTpsClients+numBenchExchangeClients))
if [[ "$numClientsRequested" -eq 0 ]]; then
  numBenchTpsClients=$numClients
  numClientsRequested=$((numBenchTpsClients+numBenchExchangeClients))
else
  if [[ "$numClientsRequested" -gt "$numClients" ]]; then
    echo "Error: More clients requested ($numClientsRequested) then available ($numClients)"
    exit 1
  fi
fi

annotate() {
  [[ -z $BUILDKITE ]] || {
    buildkite-agent annotate "$@"
  }
}

annotateBlockexplorerUrl() {
  declare blockstreamer=${blockstreamerIpList[0]}

  if [[ -n $blockstreamer ]]; then
    annotate --style info --context blockexplorer-url "Block explorer: http://$blockstreamer/"
  fi
}

build() {
  supported=("18.04")
  declare MAYBE_DOCKER=
  if [[ $(uname) != Linux || ! " ${supported[*]} " =~ $(lsb_release -sr) ]]; then
    # shellcheck source=ci/rust-version.sh
    source "$SOLANA_ROOT"/ci/rust-version.sh
    MAYBE_DOCKER="ci/docker-run.sh $rust_stable_docker_image"
  fi
  SECONDS=0
  (
    cd "$SOLANA_ROOT"
    echo "--- Build started at $(date)"

    set -x
    rm -rf farf

    buildVariant=
    if $debugBuild; then
      buildVariant=debug
    fi

    $MAYBE_DOCKER bash -c "
      set -ex
      scripts/cargo-install-all.sh farf \"$buildVariant\"
      if [[ -n \"$customPrograms\" ]]; then
        scripts/cargo-install-custom-programs.sh farf $customPrograms
      fi
    "
  )
  echo "Build took $SECONDS seconds"
}

startCommon() {
  declare ipAddress=$1
  test -d "$SOLANA_ROOT"
  if $skipSetup; then
    ssh "${sshOptions[@]}" "$ipAddress" "
      set -x;
      mkdir -p ~/solana/config;
      rm -rf ~/config;
      mv ~/solana/config ~;
      rm -rf ~/solana;
      mkdir -p ~/solana ~/.cargo/bin;
      mv ~/config ~/solana/
    "
  else
    ssh "${sshOptions[@]}" "$ipAddress" "
      set -x;
      rm -rf ~/solana;
      mkdir -p ~/.cargo/bin
    "
  fi
  [[ -z "$externalNodeSshKey" ]] || ssh-copy-id -f -i "$externalNodeSshKey" "${sshOptions[@]}" "solana@$ipAddress"
  rsync -vPrc -e "ssh ${sshOptions[*]}" \
    "$SOLANA_ROOT"/{fetch-perf-libs.sh,scripts,net,multinode-demo} \
    "$ipAddress":~/solana/
}

startBootstrapLeader() {
  declare ipAddress=$1
  declare nodeIndex="$2"
  declare logFile="$3"
  echo "--- Starting bootstrap leader: $ipAddress"
  echo "start log: $logFile"

  # Deploy local binaries to bootstrap fullnode.  Other fullnodes and clients later fetch the
  # binaries from it
  (
    set -x
    startCommon "$ipAddress" || exit 1
    [[ -z "$externalPrimordialAccountsFile" ]] || rsync -vPrc -e "ssh ${sshOptions[*]}" "$externalPrimordialAccountsFile" \
      "$ipAddress:$remoteExternalPrimordialAccountsFile"
    case $deployMethod in
    tar)
      rsync -vPrc -e "ssh ${sshOptions[*]}" "$SOLANA_ROOT"/solana-release/bin/* "$ipAddress:~/.cargo/bin/"
      rsync -vPrc -e "ssh ${sshOptions[*]}" "$SOLANA_ROOT"/solana-release/version.yml "$ipAddress:~/"
      ;;
    local)
      rsync -vPrc -e "ssh ${sshOptions[*]}" "$SOLANA_ROOT"/farf/bin/* "$ipAddress:~/.cargo/bin/"
      ssh "${sshOptions[@]}" -n "$ipAddress" "rm -f ~/version.yml; touch ~/version.yml"
      ;;
    skip)
      ;;
    *)
      usage "Internal error: invalid deployMethod: $deployMethod"
      ;;
    esac

    ssh "${sshOptions[@]}" -n "$ipAddress" \
      "./solana/net/remote/remote-node.sh \
         $deployMethod \
         bootstrap-leader \
         $entrypointIp \
         $((${#fullnodeIpList[@]} + ${#blockstreamerIpList[@]} + ${#replicatorIpList[@]})) \
         \"$RUST_LOG\" \
         $skipSetup \
         $failOnValidatorBootupFailure \
         \"$remoteExternalPrimordialAccountsFile\" \
         \"$maybeDisableAirdrops\" \
         \"$internalNodesStakeLamports\" \
         \"$internalNodesLamports\" \
         $nodeIndex \
         $numBenchTpsClients \"$benchTpsExtraArgs\" \
         $numBenchExchangeClients \"$benchExchangeExtraArgs\" \
         \"$genesisOptions\" \
         \"$maybeNoSnapshot $maybeSkipLedgerVerify $maybeLimitLedgerSize\" \
         \"$gpuMode\" \
      "
  ) >> "$logFile" 2>&1 || {
    cat "$logFile"
    echo "^^^ +++"
    exit 1
  }
}

startNode() {
  declare ipAddress=$1
  declare nodeType=$2
  declare nodeIndex="$3"
  declare logFile="$netLogDir/fullnode-$ipAddress.log"

  if [[ -z $nodeType ]]; then
    echo nodeType not specified
    exit 1
  fi

  if [[ -z $nodeIndex ]]; then
    echo nodeIndex not specified
    exit 1
  fi

  echo "--- Starting $nodeType: $ipAddress"
  echo "start log: $logFile"
  (
    set -x
    startCommon "$ipAddress"

    if [[ $nodeType = blockstreamer ]] && [[ -n $letsEncryptDomainName ]]; then
      #
      # Create/renew TLS certificate
      #
      declare localArchive=~/letsencrypt-"$letsEncryptDomainName".tgz
      if [[ -r "$localArchive" ]]; then
        timeout 30s scp "${sshOptions[@]}" "$localArchive" "$ipAddress:letsencrypt.tgz"
      fi
      ssh "${sshOptions[@]}" -n "$ipAddress" \
        "sudo -H /certbot-restore.sh $letsEncryptDomainName maintainers@solana.com"
      rm -f letsencrypt.tgz
      timeout 30s scp "${sshOptions[@]}" "$ipAddress:/letsencrypt.tgz" letsencrypt.tgz
      test -s letsencrypt.tgz # Ensure non-empty before overwriting $localArchive
      cp letsencrypt.tgz "$localArchive"
    fi

    ssh "${sshOptions[@]}" -n "$ipAddress" \
      "./solana/net/remote/remote-node.sh \
         $deployMethod \
         $nodeType \
         $entrypointIp \
         $((${#fullnodeIpList[@]} + ${#blockstreamerIpList[@]} + ${#replicatorIpList[@]})) \
         \"$RUST_LOG\" \
         $skipSetup \
         $failOnValidatorBootupFailure \
         \"$remoteExternalPrimordialAccountsFile\" \
         \"$maybeDisableAirdrops\" \
         \"$internalNodesStakeLamports\" \
         \"$internalNodesLamports\" \
         $nodeIndex \
         $numBenchTpsClients \"$benchTpsExtraArgs\" \
         $numBenchExchangeClients \"$benchExchangeExtraArgs\" \
         \"$genesisOptions\" \
         \"$maybeNoSnapshot $maybeSkipLedgerVerify $maybeLimitLedgerSize\" \
         \"$gpuMode\" \
      "
  ) >> "$logFile" 2>&1 &
  declare pid=$!
  ln -sf "fullnode-$ipAddress.log" "$netLogDir/fullnode-$pid.log"
  pids+=("$pid")
}

startClient() {
  declare ipAddress=$1
  declare clientToRun="$2"
  declare clientIndex="$3"
  declare logFile="$netLogDir/client-$clientToRun-$ipAddress.log"
  echo "--- Starting client: $ipAddress - $clientToRun"
  echo "start log: $logFile"
  (
    set -x
    startCommon "$ipAddress"
    ssh "${sshOptions[@]}" -f "$ipAddress" \
      "./solana/net/remote/remote-client.sh $deployMethod $entrypointIp \
      $clientToRun \"$RUST_LOG\" \"$benchTpsExtraArgs\" \"$benchExchangeExtraArgs\" $clientIndex"
  ) >> "$logFile" 2>&1 || {
    cat "$logFile"
    echo "^^^ +++"
    exit 1
  }
}

sanity() {
  declare skipBlockstreamerSanity=$1

  $metricsWriteDatapoint "testnet-deploy net-sanity-begin=1"

  declare ok=true
  declare bootstrapLeader=${fullnodeIpList[0]}
  declare blockstreamer=${blockstreamerIpList[0]}

  annotateBlockexplorerUrl

  echo "--- Sanity: $bootstrapLeader"
  (
    set -x
    # shellcheck disable=SC2029 # remote-client.sh args are expanded on client side intentionally
    ssh "${sshOptions[@]}" "$bootstrapLeader" \
      "./solana/net/remote/remote-sanity.sh $bootstrapLeader $sanityExtraArgs \"$RUST_LOG\""
  ) || ok=false
  $ok || exit 1

  if [[ -z $skipBlockstreamerSanity && -n $blockstreamer ]]; then
    # If there's a blockstreamer node run a reduced sanity check on it as well
    echo "--- Sanity: $blockstreamer"
    (
      set -x
      # shellcheck disable=SC2029 # remote-client.sh args are expanded on client side intentionally
      ssh "${sshOptions[@]}" "$blockstreamer" \
        "./solana/net/remote/remote-sanity.sh $blockstreamer $sanityExtraArgs -o noValidatorSanity \"$RUST_LOG\""
    ) || ok=false
    $ok || exit 1
  fi

  $metricsWriteDatapoint "testnet-deploy net-sanity-complete=1"
}

deployUpdate() {
  if [[ -z $updatePlatforms ]]; then
    echo "No update platforms"
    return
  fi
  if [[ -z $releaseChannel ]]; then
    echo "Release channel not specified (use -t option)"
    exit 1
  fi

  declare ok=true
  declare bootstrapLeader=${fullnodeIpList[0]}

  for updatePlatform in $updatePlatforms; do
    echo "--- Deploying solana-install update: $updatePlatform"
    (
      set -x

      scripts/solana-install-update-manifest-keypair.sh "$updatePlatform"

      timeout 30s scp "${sshOptions[@]}" \
        update_manifest_keypair.json "$bootstrapLeader:solana/update_manifest_keypair.json"

      # shellcheck disable=SC2029 # remote-deploy-update.sh args are expanded on client side intentionally
      ssh "${sshOptions[@]}" "$bootstrapLeader" \
        "./solana/net/remote/remote-deploy-update.sh $releaseChannel $updatePlatform"
    ) || ok=false
    $ok || exit 1
  done
}

getNodeType() {
  echo "getNodeType: $nodeAddress"
  [[ -n $nodeAddress ]] || {
    echo "Error: nodeAddress not set"
    exit 1
  }
  nodeIndex=0 # <-- global
  nodeType=validator # <-- global

  for ipAddress in "${fullnodeIpList[@]}" b "${blockstreamerIpList[@]}" r "${replicatorIpList[@]}"; do
    if [[ $ipAddress = b ]]; then
      nodeType=blockstreamer
      continue
    elif [[ $ipAddress = r ]]; then
      nodeType=replicator
      continue
    fi

    if [[ $ipAddress = "$nodeAddress" ]]; then
      echo "getNodeType: $nodeType ($nodeIndex)"
      return
    fi
    ((nodeIndex = nodeIndex + 1))
  done

  echo "Error: Unknown node: $nodeAddress"
  exit 1
}

prepare_deploy() {
  case $deployMethod in
  tar)
    if [[ -n $releaseChannel ]]; then
      rm -f "$SOLANA_ROOT"/solana-release.tar.bz2
      declare updateDownloadUrl=http://release.solana.com/"$releaseChannel"/solana-release-x86_64-unknown-linux-gnu.tar.bz2
      (
        set -x
        curl --retry 5 --retry-delay 2 --retry-connrefused \
          -o "$SOLANA_ROOT"/solana-release.tar.bz2 "$updateDownloadUrl"
      )
      tarballFilename="$SOLANA_ROOT"/solana-release.tar.bz2
    fi
    (
      set -x
      rm -rf "$SOLANA_ROOT"/solana-release
      (cd "$SOLANA_ROOT"; tar jxv) < "$tarballFilename"
      cat "$SOLANA_ROOT"/solana-release/version.yml
    )
    ;;
  local)
    if $doBuild; then
      build
    else
      echo "Build skipped due to --no-build"
    fi
    ;;
  skip)
    ;;
  *)
    usage "Internal error: invalid deployMethod: $deployMethod"
    ;;
  esac

  if [[ -n $deployIfNewer ]]; then
    if [[ $deployMethod != tar ]]; then
      echo "Error: --deploy-if-newer only supported for tar deployments"
      exit 1
    fi

    echo "Fetching current software version"
    (
      set -x
      rsync -vPrc -e "ssh ${sshOptions[*]}" "${fullnodeIpList[0]}":~/version.yml current-version.yml
    )
    cat current-version.yml
    if ! diff -q current-version.yml "$SOLANA_ROOT"/solana-release/version.yml; then
      echo "Cluster software version is old.  Update required"
    else
      echo "Cluster software version is current.  No update required"
      exit 0
    fi
  fi
}

deploy() {
  echo "Deployment started at $(date)"
  $metricsWriteDatapoint "testnet-deploy net-start-begin=1"

  declare bootstrapLeader=true
  for nodeAddress in "${fullnodeIpList[@]}" "${blockstreamerIpList[@]}" "${replicatorIpList[@]}"; do
    nodeType=
    nodeIndex=
    getNodeType
    if $bootstrapLeader; then
      SECONDS=0
      declare bootstrapNodeDeployTime=
      startBootstrapLeader "$nodeAddress" $nodeIndex "$netLogDir/bootstrap-leader-$ipAddress.log"
      bootstrapNodeDeployTime=$SECONDS
      $metricsWriteDatapoint "testnet-deploy net-bootnode-leader-started=1"

      bootstrapLeader=false
      SECONDS=0
      pids=()
    else
      startNode "$ipAddress" $nodeType $nodeIndex

      # Stagger additional node start time. If too many nodes start simultaneously
      # the bootstrap node gets more rsync requests from the additional nodes than
      # it can handle.
      if ((nodeIndex % 2 == 0)); then
        sleep 2
      fi
    fi
  done


  for pid in "${pids[@]}"; do
    declare ok=true
    wait "$pid" || ok=false
    if ! $ok; then
      echo "+++ fullnode failed to start"
      cat "$netLogDir/fullnode-$pid.log"
      if $failOnValidatorBootupFailure; then
        exit 1
      else
        echo "Failure is non-fatal"
      fi
    fi
  done

  $metricsWriteDatapoint "testnet-deploy net-fullnodes-started=1"
  additionalNodeDeployTime=$SECONDS

  annotateBlockexplorerUrl

  sanity skipBlockstreamerSanity # skip sanity on blockstreamer node, it may not
                                 # have caught up to the bootstrap leader yet

  SECONDS=0
  for ((i=0; i < "$numClients" && i < "$numClientsRequested"; i++)) do
    if [[ $i -lt "$numBenchTpsClients" ]]; then
      startClient "${clientIpList[$i]}" "solana-bench-tps" "$i"
    else
      startClient "${clientIpList[$i]}" "solana-bench-exchange" $((i-numBenchTpsClients))
    fi
  done
  clientDeployTime=$SECONDS

  $metricsWriteDatapoint "testnet-deploy net-start-complete=1"

  declare networkVersion=unknown
  case $deployMethod in
  tar)
    networkVersion="$(
      (
        set -o pipefail
        grep "^commit: " "$SOLANA_ROOT"/solana-release/version.yml | head -n1 | cut -d\  -f2
      ) || echo "tar-unknown"
    )"
    ;;
  local)
    networkVersion="$(git rev-parse HEAD || echo local-unknown)"
    ;;
  skip)
    ;;
  *)
    usage "Internal error: invalid deployMethod: $deployMethod"
    ;;
  esac
  $metricsWriteDatapoint "testnet-deploy version=\"${networkVersion:0:9}\""

  echo
  echo "+++ Deployment Successful"
  echo "Bootstrap leader deployment took $bootstrapNodeDeployTime seconds"
  echo "Additional fullnode deployment (${#fullnodeIpList[@]} full nodes, ${#blockstreamerIpList[@]} blockstreamer nodes, ${#replicatorIpList[@]} replicators) took $additionalNodeDeployTime seconds"
  echo "Client deployment (${#clientIpList[@]} instances) took $clientDeployTime seconds"
  echo "Network start logs in $netLogDir"
}


stopNode() {
  local ipAddress=$1
  local block=$2
  declare logFile="$netLogDir/stop-fullnode-$ipAddress.log"
  echo "--- Stopping node: $ipAddress"
  echo "stop log: $logFile"
  (
    set -x
    # shellcheck disable=SC2029 # It's desired that PS4 be expanded on the client side
    ssh "${sshOptions[@]}" "$ipAddress" "
      PS4=\"$PS4\"
      set -x
      ! tmux list-sessions || tmux kill-session
      for pid in solana/{blockexplorer,net-stats,fd-monitor,oom-monitor}.pid; do
        pgid=\$(ps opgid= \$(cat \$pid) | tr -d '[:space:]')
        if [[ -n \$pgid ]]; then
          sudo kill -- -\$pgid
        fi
      done
      for pattern in node solana- remote-; do
        pkill -9 \$pattern
      done
    "
  ) >> "$logFile" 2>&1 &

  declare pid=$!
  ln -sf "stop-fullnode-$ipAddress.log" "$netLogDir/stop-fullnode-$pid.log"
  if $block; then
    wait $pid
  else
    pids+=("$pid")
  fi
}

stop() {
  SECONDS=0
  $metricsWriteDatapoint "testnet-deploy net-stop-begin=1"

  declare loopCount=0
  pids=()
  for ipAddress in "${fullnodeIpList[@]}" "${blockstreamerIpList[@]}" "${replicatorIpList[@]}" "${clientIpList[@]}"; do
    stopNode "$ipAddress" false

    # Stagger additional node stop time to avoid too many concurrent ssh
    # sessions
    ((loopCount++ % 4 == 0)) && sleep 2
  done

  echo --- Waiting for nodes to finish stopping
  for pid in "${pids[@]}"; do
    echo -n "$pid "
    wait "$pid" || true
  done
  echo

  $metricsWriteDatapoint "testnet-deploy net-stop-complete=1"
  echo "Stopping nodes took $SECONDS seconds"
}


checkPremptibleInstances() {
  # The fullnodeIpList nodes may be preemptible instances that can disappear at
  # any time.  Try to detect when a fullnode has been preempted to help the user
  # out.
  #
  # Of course this isn't airtight as an instance could always disappear
  # immediately after its successfully pinged.
  for ipAddress in "${fullnodeIpList[@]}"; do
    (
      set -x
      timeout 5s ping -c 1 "$ipAddress"
    ) || {
      cat <<EOF

Warning: $ipAddress may have been preempted.

Run |./gce.sh config| to restart it
EOF
      exit 1
    }
  done
}

checkPremptibleInstances

case $command in
restart)
  prepare_deploy
  stop
  deploy
  ;;
start)
  prepare_deploy
  deploy
  ;;
sanity)
  sanity
  ;;
stop)
  stop
  ;;
update)
  deployUpdate
  ;;
stopnode)
  if [[ -z $nodeAddress ]]; then
    usage "node address (-i) not specified"
    exit 1
  fi
  stopNode "$nodeAddress" true
  ;;
startnode)
  if [[ -z $nodeAddress ]]; then
    usage "node address (-i) not specified"
    exit 1
  fi
  nodeType=
  nodeIndex=
  getNodeType
  startNode "$nodeAddress" $nodeType $nodeIndex
  ;;
logs)
  fetchRemoteLog() {
    declare ipAddress=$1
    declare log=$2
    echo "--- fetching $log from $ipAddress"
    (
      set -x
      timeout 30s scp "${sshOptions[@]}" \
        "$ipAddress":solana/"$log".log "$netLogDir"/remote-"$log"-"$ipAddress".log
    ) || echo "failed to fetch log"
  }
  fetchRemoteLog "${fullnodeIpList[0]}" drone
  for ipAddress in "${fullnodeIpList[@]}"; do
    fetchRemoteLog "$ipAddress" fullnode
  done
  for ipAddress in "${clientIpList[@]}"; do
    fetchRemoteLog "$ipAddress" client
  done
  for ipAddress in "${blockstreamerIpList[@]}"; do
    fetchRemoteLog "$ipAddress" fullnode
  done
  for ipAddress in "${replicatorIpList[@]}"; do
    fetchRemoteLog "$ipAddress" fullnode
  done
  ;;

*)
  echo "Internal error: Unknown command: $command"
  usage
  exit 1
esac
