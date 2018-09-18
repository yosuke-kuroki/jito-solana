#!/bin/bash -e
#
# This script is to be run on the leader node
#

cd "$(dirname "$0")"/../..

deployMethod=
entrypointIp=
numNodes=

[[ -r deployConfig ]] || {
  echo deployConfig missing
  exit 1
}
# shellcheck source=/dev/null # deployConfig is written by remote-node.sh
source deployConfig

missing() {
  echo "Error: $1 not specified"
  exit 1
}

[[ -n $deployMethod ]] || missing deployMethod
[[ -n $entrypointIp ]] || missing entrypointIp
[[ -n $numNodes ]]     || missing numNodes

ledgerVerify=true
validatorSanity=true
rejectExtraNodes=false
while [[ $1 = -o ]]; do
  opt="$2"
  shift 2
  case $opt in
  noLedgerVerify)
    ledgerVerify=false
    ;;
  noValidatorSanity)
    validatorSanity=false
    ;;
  rejectExtraNodes)
    rejectExtraNodes=true
    ;;
  *)
    echo "Error: unknown option: $opt"
    exit 1
    ;;
  esac
done

source net/common.sh
loadConfigFile

case $deployMethod in
snap)
  PATH="/snap/bin:$PATH"
  export USE_SNAP=1
  entrypointRsyncUrl="$entrypointIp"

  solana_bench_tps=solana.bench-tps
  solana_ledger_tool=solana.ledger-tool
  solana_keygen=solana.keygen

  ledger=/var/snap/solana/current/config/ledger
  client_id=~/snap/solana/current/config/client-id.json

  ;;
local)
  PATH="$HOME"/.cargo/bin:"$PATH"
  export USE_INSTALL=1
  entrypointRsyncUrl="$entrypointIp:~/solana"

  solana_bench_tps=solana-bench-tps
  solana_ledger_tool=solana-ledger-tool
  solana_keygen=solana-keygen

  ledger=config/ledger
  client_id=config/client-id.json
  ;;
*)
  echo "Unknown deployment method: $deployMethod"
  exit 1
esac


echo "--- $entrypointIp: wallet sanity"
(
  set -x
  scripts/wallet-sanity.sh "$entrypointIp:8001"
)

echo "+++ $entrypointIp: node count ($numNodes expected)"
(
  set -x
  $solana_keygen -o "$client_id"

  maybeRejectExtraNodes=
  if $rejectExtraNodes; then
    maybeRejectExtraNodes="--reject-extra-nodes"
  fi

  $solana_bench_tps \
    --network "$entrypointIp:8001" \
    --identity "$client_id" \
    --num-nodes "$numNodes" \
    $maybeRejectExtraNodes \
    --converge-only
)

echo "--- $entrypointIp: verify ledger"
if $ledgerVerify; then
  if [[ -d $ledger ]]; then
    (
      set -x
      rm -rf /var/tmp/ledger-verify
      du -hs "$ledger"
      time cp -r "$ledger" /var/tmp/ledger-verify
      time $solana_ledger_tool --ledger /var/tmp/ledger-verify verify
    )
  else
    echo "^^^ +++"
    echo "Ledger verify skipped: directory does not exist: $ledger"
  fi
else
  echo "^^^ +++"
  echo "Note: ledger verify disabled"
fi


echo "--- $entrypointIp: validator sanity"
if $validatorSanity; then
  (
    set -ex -o pipefail
    ./multinode-demo/setup.sh -t validator
    timeout 10s ./multinode-demo/validator.sh "$entrypointRsyncUrl" "$entrypointIp:8001" 2>&1 | tee validator.log
  ) || {
    exitcode=$?
    [[ $exitcode -eq 124 ]] || exit $exitcode
  }
  wc -l validator.log
  if grep -C100 panic validator.log; then
    echo "^^^ +++"
    echo "Panic observed"
    exit 1
  else
    echo "Validator log looks ok"
  fi
else
  echo "^^^ +++"
  echo "Note: validator sanity disabled"
fi

echo --- Pass
