# Install the Solana software

Before attempting to connect your validator to the Tour de SOL cluster, be familiar with connecting a validator to the Public Testnet as described [here](../../../running-validator/README.md).

You can confirm the version running on the cluster entrypoint by running:

```text
curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc": "2.0","id":1,"method":"getVersion"}' tds.solana.com:8899
```

Note the version number

## Install Software

Install the Solana release [LATEST_SOLANA_RELEASE_VERSION](https://github.com/solana-labs/solana/releases/tag/LATEST_SOLANA_RELEASE_VERSION) on your machine by running:

```bash
curl -sSf https://raw.githubusercontent.com/solana-labs/solana/v1.0.0/install/solana-install-init.sh | sh -s - LATEST_SOLANA_RELEASE_VERSION
```

then run `solana --version` to confirm the expected version number.
