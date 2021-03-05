#!/usr/bin/env bash
set -e

cd "$(dirname "$0")/.."
# shellcheck source=multinode-demo/common.sh
source multinode-demo/common.sh

rm -rf config/run/init-completed config/ledger config/snapshot-ledger

timeout 120 ./run.sh &
pid=$!

attempts=20
while [[ ! -f config/run/init-completed ]]; do
  sleep 1
  if ((--attempts == 0)); then
     echo "Error: validator failed to boot"
     exit 1
  fi
done

snapshot_slot=1

# wait a bit longer than snapshot_slot
while [[ $($solana_cli --url http://localhost:8899 slot --commitment recent) -le $((snapshot_slot + 1)) ]]; do
  sleep 1
done

$solana_validator --ledger config/ledger exit || true

wait $pid

$solana_ledger_tool create-snapshot --ledger config/ledger "$snapshot_slot" config/snapshot-ledger
cp config/ledger/genesis.tar.bz2 config/snapshot-ledger
$solana_ledger_tool verify --ledger config/snapshot-ledger
