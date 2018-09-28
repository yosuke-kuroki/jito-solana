#!/bin/bash -e

cd "$(dirname "$0")/.."

usage() {
  exitcode=0
  if [[ -n "$1" ]]; then
    exitcode=1
    echo "Error: $*"
  fi
  cat <<EOF
usage: $0 [name] [zone]

Sanity check a CD testnet

  name  - name of the network
  zone  - zone of the network

  Note: the SOLANA_METRICS_CONFIG environment variable is used to configure
        metrics
EOF
  exit $exitcode
}

netName=$1
zone=$2
[[ -n $netName ]] || usage ""
[[ -n $zone ]] || usage "Zone not specified"

set -x
echo --- gce.sh config
net/gce.sh config -p "$netName" -z "$zone"
net/init-metrics.sh -e
echo --- net.sh sanity
net/net.sh sanity \
  ${NO_LEDGER_VERIFY:+-o noLedgerVerify} \
  ${NO_VALIDATOR_SANITY:+-o noValidatorSanity} \
  ${REJECT_EXTRA_NODES:+-o rejectExtraNodes} \

exit 0
