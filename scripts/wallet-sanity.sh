#!/bin/bash -e
#
# Wallet sanity test
#

cd "$(dirname "$0")"/..

# shellcheck source=multinode-demo/common.sh
source multinode-demo/common.sh

if [[ -z $1 ]]; then # no network argument, use default
  entrypoint=()
else
  entrypoint=(-n "$1")
fi

# Tokens transferred to this address are lost forever...
garbage_address=vS3ngn1TfQmpsW1Z4NkLuqNAQFF3dYQw8UZ6TCx9bmq

check_balance_output() {
  declare expected_output="$1"
  exec 42>&1
  output=$($solana_wallet "${entrypoint[@]}" balance | tee >(cat - >&42))
  if [[ ! "$output" =~ $expected_output ]]; then
    echo "Balance is incorrect.  Expected: $expected_output"
    exit 1
  fi
}

pay_and_confirm() {
  exec 42>&1
  signature=$($solana_wallet "${entrypoint[@]}" pay "$@" | tee >(cat - >&42))
  $solana_wallet "${entrypoint[@]}" confirm "$signature"
}

$solana_keygen
$solana_wallet "${entrypoint[@]}" address
check_balance_output "No account found" "Your balance is: 0"
$solana_wallet "${entrypoint[@]}" airdrop 60
check_balance_output "Your balance is: 60"
$solana_wallet "${entrypoint[@]}" airdrop 40
check_balance_output "Your balance is: 100"
pay_and_confirm $garbage_address 99
check_balance_output "Your balance is: 1"

echo PASS
exit 0
