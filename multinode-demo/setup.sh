#!/usr/bin/env bash

here=$(dirname "$0")
# shellcheck source=multinode-demo/common.sh
source "$here"/common.sh
setup_secondary_mount

set -e

rm -rf "$SOLANA_CONFIG_DIR"/bootstrap-leader
mkdir -p "$SOLANA_CONFIG_DIR"/bootstrap-leader

# Create genesis ledger
$solana_keygen new -f -o "$SOLANA_CONFIG_DIR"/mint-keypair.json

$solana_keygen new -o "$SOLANA_CONFIG_DIR"/bootstrap-leader/identity-keypair.json
$solana_keygen new -o "$SOLANA_CONFIG_DIR"/bootstrap-leader/vote-keypair.json
$solana_keygen new -o "$SOLANA_CONFIG_DIR"/bootstrap-leader/stake-keypair.json
$solana_keygen new -o "$SOLANA_CONFIG_DIR"/bootstrap-leader/storage-keypair.json

args=("$@")
default_arg --bootstrap-leader-keypair "$SOLANA_CONFIG_DIR"/bootstrap-leader/identity-keypair.json
default_arg --bootstrap-vote-keypair "$SOLANA_CONFIG_DIR"/bootstrap-leader/vote-keypair.json
default_arg --bootstrap-stake-keypair "$SOLANA_CONFIG_DIR"/bootstrap-leader/stake-keypair.json
default_arg --bootstrap-storage-keypair "$SOLANA_CONFIG_DIR"/bootstrap-leader/storage-keypair.json
default_arg --ledger "$SOLANA_CONFIG_DIR"/bootstrap-leader
default_arg --mint "$SOLANA_CONFIG_DIR"/mint-keypair.json
default_arg --hashes-per-tick auto
default_arg --dev
$solana_genesis "${args[@]}"

(
  cd "$SOLANA_CONFIG_DIR"/bootstrap-leader
  set -x
  tar jcvfS genesis.tar.bz2 genesis.bin rocksdb
)
