#!/usr/bin/env bash
set -e

cd "$(dirname "$0")/.."
source ci/upload-ci-artifact.sh

usage() {
  exitcode=0
  if [[ -n "$1" ]]; then
    exitcode=1
    echo "Error: $*"
  fi
  cat <<EOF
usage: $0 [name] [cloud] [zone]

Sanity check a CD testnet

  name  - name of the network
  cloud - cloud provider to use (gce, ec2)
  zone  - cloud provider zone of the network

  Note: the SOLANA_METRICS_CONFIG environment variable is used to configure
        metrics
EOF
  exit $exitcode
}

netName=$1
cloudProvider=$2
zone=$3
[[ -n $netName ]] || usage ""
[[ -n $cloudProvider ]] || usage "Cloud provider not specified"
[[ -n $zone ]] || usage "Zone not specified"

shutdown() {
  exitcode=$?

  set +e
  for logfile in net/log/*; do
    if [[ -f $logfile ]]; then
      upload-ci-artifact "$logfile"
      tail "$logfile"
    fi
  done

  exit $exitcode
}

trap shutdown EXIT INT

set -x
echo "--- $cloudProvider.sh config"
timeout 5m net/"$cloudProvider".sh config -p "$netName" -z "$zone"
net/init-metrics.sh -e
echo --- net.sh sanity
timeout 5m net/net.sh sanity \
  ${NO_LEDGER_VERIFY:+-o noLedgerVerify} \
  ${NO_VALIDATOR_SANITY:+-o noValidatorSanity} \
  ${REJECT_EXTRA_NODES:+-o rejectExtraNodes} \

exit 0
