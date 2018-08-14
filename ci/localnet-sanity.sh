#!/bin/bash -e
#
# Perform a quick sanity test on a leader, drone, validator and client running
# locally on the same machine
#

cd "$(dirname "$0")"/..
source ci/upload_ci_artifact.sh
source multinode-demo/common.sh

./multinode-demo/setup.sh

backgroundCommands="drone leader validator validator-x"
pids=()

for cmd in $backgroundCommands; do
  echo "--- Start $cmd"
  rm -f log-"$cmd".txt
  ./multinode-demo/"$cmd".sh > log-"$cmd".txt 2>&1 &
  declare pid=$!
  pids+=("$pid")
  echo "pid: $pid"
done

killBackgroundCommands() {
  set +e
  for pid in "${pids[@]}"; do
    if kill "$pid"; then
      wait "$pid"
    else
      echo -e "^^^ +++\\nWarning: unable to kill $pid"
    fi
  done
  set -e
  pids=()
}

shutdown() {
  exitcode=$?
  killBackgroundCommands

  set +e

  echo "--- Upload artifacts"
  for cmd in $backgroundCommands; do
    declare logfile=log-$cmd.txt
    upload_ci_artifact "$logfile"
    tail "$logfile"
  done

  exit $exitcode
}

trap shutdown EXIT INT

set -e

flag_error() {
  echo Failed
  echo "^^^ +++"
  exit 1
}

echo "--- Wallet sanity"
(
  set -x
  multinode-demo/test/wallet-sanity.sh
) || flag_error

echo "--- Node count"
(
  set -x
  ./multinode-demo/client.sh "$PWD" 3 -c --addr 127.0.0.1
) || flag_error

killBackgroundCommands

echo "--- Ledger verification"
(
  set -x
  $solana_ledger_tool --ledger "$SOLANA_CONFIG_DIR"/ledger verify
) || flag_error

echo +++
echo Ok
exit 0
