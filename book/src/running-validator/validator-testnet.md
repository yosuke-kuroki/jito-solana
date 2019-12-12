# Choosing a Testnet

Solana maintains several testnets, each featuring a Solana-owned validator
that serves as an entrypoint to the cluster.

Current testnet entrypoints:

* Stable: testnet.solana.com
* Beta: beta.testnet.solana.com
* Edge: edge.testnet.solana.com

Application developers should target the Stable testnet. Key differences
between the Stable testnet and what will be mainnet:

* Stable testnet tokens are not real
* Stable testnet includes a token faucet for application testing
* Stable testnet may be subject to ledger resets
* Stable testnet typically runs a newer software version than mainnet
* Stable testnet may be maintained by different validators than mainnet

The Beta testnet is used to showcase and stabilize new features before they
are marked Stable. Application developers are free to target the Beta testnet,
but should expect instability and periodic ledger resets.

The Edge testnet is intended for Solana protocol developers, not application
developers. It tracks the tip of the master branch, not any release. Regarding
stability, all that can be said is that CI automation was successful.

### Get Testnet Version

You can submit a JSON-RPC request to see the specific software version of the
cluster. Use this to specify [the software version to install](validator-software.md).

```bash
curl -X POST -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1, "method":"getVersion"}' testnet.solana.com:8899
```
Example result:
`{"jsonrpc":"2.0","result":{"solana-core":"0.21.0"},"id":1}`

## Using a Different Testnet

This guide is written in the context of testnet.solana.com, our most stable
cluster. To participate in another testnet, modify the commands in the following
pages, replacing `testnet.solana.com` with your desired testnet.
