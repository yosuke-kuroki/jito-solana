## Testnet Participation
This document describes how to participate in a public testnet as a
validator node using the *Beacons v0.12* release.

Please note some of the information and instructions described here may change
in future releases.

### Beta Testnet Overview
The beta testnet features a validator running at beta.testnet.solana.com, which
serves as the entrypoint to the cluster for your validator.

Additionally there is a blockexplorer available at http://beta.testnet.solana.com/.

The beta testnet is configured to reset the ledger every 24hours, or sooner
should an hourly automated sanity test fail.

### Machine Requirements
Since the beta testnet is not intended for stress testing of max transaction
throughput, a higher-end machine with a GPU is not necessary to participate.

However ensure the machine used is not behind a residential NAT to avoid NAT
traversal issues.  A cloud-hosted machine works best.  Ensure that IP ports
8000 through 10000 are not blocked for Internet traffic.

Prebuilt binaries are available for Linux x86_64 (Ubuntu 18.04 recommended).
MacOS or WSL users may build from source.

### Validator Setup
The shell commands in this section assume the following environment variables are
set:
```bash
$ export release=0.12.1
$ export ip=$(dig +short beta.testnet.solana.com)
```

#### Obtaining The Software
Prebuilt binaries are available for Linux x86_64 systems.  Download and install by running:
```bash
$ wget https://github.com/solana-labs/solana/releases/download/v${release:?}/solana-release-x86_64-unknown-linux-gnu.tar.bz2 -O solana-release.tar.gz
$ tar jxf solana-release.tar.bz2
$ cd solana-release/
$ export PATH=$PWD/bin:$PATH
```

If you are unable to use the prebuilt binaries or prefer to build it yourself from source:
```bash
$ wget https://github.com/solana-labs/solana/archive/v${release:?}.tar.gz -O solana-release.tar.gz
$ tar zxf solana-release.tar.gz
$ cd solana-${release:?}
$ ./scripts/cargo-install-all.sh .
$ export PATH=$PWD/bin:$PATH
```

#### Confirm The Testnet Is Reachable
Before attaching a validator node, sanity check that the cluster is accessible
to your machine by running some simple wallet commands.  If any of these
commands fail, please retry 5-10 minutes later to confirm the testnet is not
just restarting itself before debugging further.

Receive an airdrop of lamports from the testnet drone:
```bash
$ solana-wallet -n ${ip:?} airdrop 123
$ solana-wallet -n ${ip:?} balance
```

Fetch the current testnet transaction count over JSON RPC:
```bash
$ curl -X POST -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1, "method":"getTransactionCount"}' http://beta.testnet.solana.com:8899
```

Inspect the blockexplorer at http://beta.testnet.solana.com/ for activity.

Run the following command to join the gossip network and view all the other nodes in the cluster:
```bash
$ RUST_LOG=info solana-bench-tps --converge-only --num-nodes 100000 --network ${ip:?}:8001
```

#### Starting The Validator
The following command will start a new validator node:
```bash
$ RUST_LOG=warn ./multinode-demo/fullnode-x.sh --public-address --poll-for-new-genesis-block ${ip:?}
```

Then from another console, confirm the IP address if your node is now visible in
the gossip network by running:
```bash
$ RUST_LOG=info solana-bench-tps --converge-only --num-nodes 100000 --network ${ip:?}:8001
```

Congratulations, you're now participating in the testnet cluster!

#### Sharing Metrics From Your Validator
If you'd like to share metrics perform the following steps before starting the
validator node:
```bash
export u="username obtained from the Solana maintainers"
export p="password obtained from the Solana maintainers"
export SOLANA_METRICS_CONFIG="db=testnet-beta,u=${u:?},p=${p:?}"
source scripts/configure-metrics.sh
```
