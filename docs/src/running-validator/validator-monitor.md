---
title: Monitoring a Validator
---

## Check Gossip

Confirm the IP address and **identity pubkey** of your validator is visible in
the gossip network by running:

```bash
solana-gossip spy --entrypoint devnet.solana.com:8001
```

## Check Your Balance

Your account balance should decrease by the transaction fee amount as your
validator submits votes, and increase after serving as the leader. Pass the
`--lamports` are to observe in finer detail:

```bash
solana balance --lamports
```

## Check Vote Activity

The `solana vote-account` command displays the recent voting activity from
your validator:

```bash
solana vote-account ~/vote-account-keypair.json
```

## Get Cluster Info

There are several useful JSON-RPC endpoints for monitoring your validator on the
cluster, as well as the health of the cluster:

```bash
# Similar to solana-gossip, you should see your validator in the list of cluster nodes
curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1, "method":"getClusterNodes"}' http://devnet.solana.com
# If your validator is properly voting, it should appear in the list of `current` vote accounts. If staked, `stake` should be > 0
curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1, "method":"getVoteAccounts"}' http://devnet.solana.com
# Returns the current leader schedule
curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1, "method":"getLeaderSchedule"}' http://devnet.solana.com
# Returns info about the current epoch. slotIndex should progress on subsequent calls.
curl -X POST -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1, "method":"getEpochInfo"}' http://devnet.solana.com
```

## Timezone For Log Messages

Log messages emitted by your validator include a timestamp. When sharing logs
with others to help triage issues, that timestamp can cause confusion as it does
not contain timezone information.

To make it easier to compare logs between different sources we request that
everybody use Pacific Time on their validator nodes. In Linux this can be
accomplished by running:

```bash
sudo ln -sf /usr/share/zoneinfo/America/Los_Angeles /etc/localtime
```
