#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"/..
source ci/upload-ci-artifact.sh

zone=
bootstrapFullNodeAddress=
bootstrapFullNodeMachineType=
clientNodeCount=0
additionalFullNodeCount=10
publicNetwork=false
skipSetup=false
skipStart=false
externalNode=false
tarChannelOrTag=edge
delete=false
enableGpu=false
bootDiskType=""
leaderRotation=true
blockstreamer=false

usage() {
  exitcode=0
  if [[ -n "$1" ]]; then
    exitcode=1
    echo "Error: $*"
  fi
  cat <<EOF
usage: $0 -p network-name -C cloud -z zone1 [-z zone2] ... [-z zoneN] [options...]

Deploys a CD testnet

  mandatory arguments:
  -p [network-name]  - name of the network
  -C [cloud] - cloud provider to use (gce, ec2)
  -z [zone]  - cloud provider zone to deploy the network into.  Must specify at least one zone

  options:
   -t edge|beta|stable|vX.Y.Z  - Deploy the latest tarball release for the
                                 specified release channel (edge|beta|stable) or release tag
                                 (vX.Y.Z)
                                 (default: $tarChannelOrTag)
   -n [number]          - Number of additional full nodes (default: $additionalFullNodeCount)
   -c [number]          - Number of client bencher nodes (default: $clientNodeCount)
   -u                   - Include a Blockstreamer (default: $blockstreamer)
   -P                   - Use public network IP addresses (default: $publicNetwork)
   -G                   - Enable GPU, and set count/type of GPUs to use (e.g n1-standard-16 --accelerator count=4,type=nvidia-tesla-k80)
   -g                   - Enable GPU (default: $enableGpu)
   -b                   - Disable leader rotation
   -a [address]         - Set the bootstrap fullnode's external IP address to this GCE address
   -d [disk-type]       - Specify a boot disk type (default None) Use pd-ssd to get ssd on GCE.
   -D                   - Delete the network
   -r                   - Reuse existing node/ledger configuration from a
                          previous |start| (ie, don't run ./multinode-demo/setup.sh).

   Note: the SOLANA_METRICS_CONFIG environment variable is used to configure
         metrics
EOF
  exit $exitcode
}

zone=()

while getopts "h?p:Pn:c:t:gG:a:Dbd:rusxz:p:C:" opt; do
  case $opt in
  h | \?)
    usage
    ;;
  p)
    netName=$OPTARG
    ;;
  C)
    cloudProvider=$OPTARG
    ;;
  z)
    zone+=("$OPTARG")
    ;;
  P)
    publicNetwork=true
    ;;
  n)
    additionalFullNodeCount=$OPTARG
    ;;
  c)
    clientNodeCount=$OPTARG
    ;;
  t)
    case $OPTARG in
    edge|beta|stable|v*)
      tarChannelOrTag=$OPTARG
      ;;
    *)
      usage "Invalid release channel: $OPTARG"
      ;;
    esac
    ;;
  b)
    leaderRotation=false
    ;;
  g)
    enableGpu=true
    ;;
  G)
    enableGpu=true
    bootstrapFullNodeMachineType=$OPTARG
    ;;
  a)
    bootstrapFullNodeAddress=$OPTARG
    ;;
  d)
    bootDiskType=$OPTARG
    ;;
  D)
    delete=true
    ;;
  r)
    skipSetup=true
    ;;
  s)
    skipStart=true
    ;;
  x)
    externalNode=true
    ;;
  u)
    blockstreamer=true
    ;;
  *)
    usage "Error: unhandled option: $opt"
    ;;
  esac
done

[[ -n $netName ]] || usage
[[ -n $cloudProvider ]] || usage "Cloud provider not specified"
[[ -n ${zone[*]} ]] || usage "At least one zone must be specified"

shutdown() {
  exitcode=$?

  set +e
  if [[ -d net/log ]]; then
    mv net/log net/log-deploy
    for logfile in net/log-deploy/*; do
      if [[ -f $logfile ]]; then
        upload-ci-artifact "$logfile"
        tail "$logfile"
      fi
    done
  fi
  exit $exitcode
}
rm -rf net/{log,-deploy}
trap shutdown EXIT INT

set -x

# Build a string to pass zone opts to $cloudProvider.sh: "-z zone1 -z zone2 ..."
zone_args=()
for val in "${zone[@]}"; do
  zone_args+=("-z $val")
done

if ! $skipSetup; then
  echo "--- $cloudProvider.sh delete"
  # shellcheck disable=SC2068
  time net/"$cloudProvider".sh delete ${zone_args[@]} -p "$netName" ${externalNode:+-x}
  if $delete; then
    exit 0
  fi

  echo "--- $cloudProvider.sh create"
  create_args=(
    -p "$netName"
    -a "$bootstrapFullNodeAddress"
    -c "$clientNodeCount"
    -n "$additionalFullNodeCount"
  )
  # shellcheck disable=SC2206
  create_args+=(${zone_args[@]})

  if $blockstreamer; then
    create_args+=(-u)
  fi

  if [[ -n $bootDiskType ]]; then
    create_args+=(-d "$bootDiskType")
  fi

  if $enableGpu; then
    if [[ -z $bootstrapFullNodeMachineType ]]; then
      create_args+=(-g)
    else
      create_args+=(-G "$bootstrapFullNodeMachineType")
    fi
  fi

  if ! $leaderRotation; then
    create_args+=(-b)
  fi

  if $publicNetwork; then
    create_args+=(-P)
  fi

  if $externalNode; then
    create_args+=(-x)
  fi

  time net/"$cloudProvider".sh create "${create_args[@]}"
else
  echo "--- $cloudProvider.sh config"
  config_args=(
    -p "$netName"
  )
  # shellcheck disable=SC2206
  config_args+=(${zone_args[@]})
  if $publicNetwork; then
    config_args+=(-P)
  fi

  time net/"$cloudProvider".sh config "${config_args[@]}"
fi
net/init-metrics.sh -e

echo "+++ $cloudProvider.sh info"
net/"$cloudProvider".sh info

echo --- net.sh start
maybeRejectExtraNodes=
if ! $publicNetwork; then
  maybeRejectExtraNodes="-o rejectExtraNodes"
fi
maybeNoValidatorSanity=
if [[ -n $NO_VALIDATOR_SANITY ]]; then
  maybeNoValidatorSanity="-o noValidatorSanity"
fi
maybeNoLedgerVerify=
if [[ -n $NO_LEDGER_VERIFY ]]; then
  maybeNoLedgerVerify="-o noLedgerVerify"
fi

maybeSkipSetup=
if $skipSetup; then
  maybeSkipSetup="-r"
fi

ok=true
if ! $skipStart; then
  (
    if $skipSetup; then
      # TODO: Enable rolling updates
      #op=update
      op=restart
    else
      op=start
    fi

    # shellcheck disable=SC2086 # Don't want to double quote maybeRejectExtraNodes
    time net/net.sh $op -t "$tarChannelOrTag" \
      $maybeSkipSetup $maybeRejectExtraNodes $maybeNoValidatorSanity $maybeNoLedgerVerify
  ) || ok=false

  net/net.sh logs
fi

$ok
