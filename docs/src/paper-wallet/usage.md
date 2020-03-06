# Paper Wallet Usage

Solana commands can be run without ever saving a keypair to disk on a machine.
If avoiding writing a private key to disk is a security concern of yours, you've
come to the right place.

{% hint style="warning" %}
Even using this secure input method, it's still possible that a private key gets
written to disk by unencrypted memory swaps. It is the user's responsibility to
protect against this scenario.
{% endhint %}

## Before You Begin

- [Install the Solana command-line tools](../install-solana.md)

### Check your installation

Check that `solana-keygen` is installed correctly by running:

```bash
solana-keygen --version
```

## Creating a Paper Wallet

Using the `solana-keygen` tool, it is possible to generate new seed phrases as
well as derive a keypair from an existing seed phrase and (optional) passphrase.
The seed phrase and passphrase can be used together as a paper wallet. As long
as you keep your seed phrase and passphrase stored safely, you can use them to
access your account.

{% hint style="info" %}
For more information about how seed phrases work, review this
[Bitcoin Wiki page](https://en.bitcoin.it/wiki/Seed_phrase).
{% endhint %}

### Seed Phrase Generation

Generating a new keypair can be done using the `solana-keygen new` command. The
command will generate a random seed phrase, ask you to enter an optional
passphrase, and then will display the derived public key and the generated seed
phrase for your paper wallet.

After copying down your seed phrase, you can use the
[public key derivation](#public-key-derivation) instructions to verify that you
have not made any errors.

```bash
solana-keygen new --no-outfile
```

{% hint style="warning" %}
If the `--no-outfile` flag is **omitted**, the default behavior is to write the
keypair to `~/.config/solana/id.json`
{% endhint %}

{% hint style="info" %}
For added security, increase the seed phrase word count using the `--word-count`
argument
{% endhint %}

For full usage details run:

```bash
solana-keygen new --help
```

### Public Key Derivation

Public keys can be derived from a seed phrase and a passphrase if you choose to
use one. This is useful for using an offline-generated seed phrase to
derive a valid public key. The `solana-keygen pubkey` command will walk you
through entering your seed phrase and a passphrase if you chose to use one.

```bash
solana-keygen pubkey ASK
```

{% hint style="info" %}
Note that you could potentially use different passphrases for the same seed
phrase. Each unique passphrase will yield a different keypair.
{% endhint %}

The `solana-keygen` tool uses the same BIP39 standard English word list as it
does to generate seed phrases. If your seed phrase was generated with another
tool that uses a different word list, you can still use `solana-keygen`, but
will need to pass the `--skip-seed-phrase-validation` argument and forego this
validation.

```bash
solana-keygen pubkey ASK --skip-seed-phrase-validation
```

{% hint style="info" %}
Copy the derived public key to a USB stick for easy usage on networked computers
{% endhint %}

{% hint style="info" %}
A common next step is to [check the balance](#checking-account-balance) of the
account associated with a public key
{% endhint %}

For full usage details run:

```bash
solana-keygen pubkey --help
```

## Verifying the Keypair

A keypair can be verified by following a variation on the
[offline signing](../offline-signing/README.md) procedure with a dummy transaction.

### Create and Sign a Dummy Transaction

Use offline signing to acquire the signature of a dummy transaction that can
be verified in the next step. A 0 Lamport [transfer](../cli/usage.md#solana-transfer)
is used to prevent inadvertent loss of funds. Additionally, an improbable _blockhash_
value is specified, as well as using the address of the _system program_ for the `TO`
argument, to ensure the transaction would be rejected by the _cluster_ should
it be submitted in error.

Command

```text
solana transfer 11111111111111111111111111111111 0 --sign-only \
    --keypair ASK --blockhash 11111111111111111111111111111111
```

Prompt for seed phrase

```text
[keypair] seed phrase:
[keypair] If this seed phrase has an associated passphrase, enter it now. Otherwise, press ENTER to continue:
Recovered pubkey `AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi`. Continue? (y/n): y
```

Output

```text
Blockhash: 11111111111111111111111111111111
Signers (Pubkey=Signature):
  AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi=3uZndChSmPoYfaCihC993E7EAHKDsuu53Ge6Dk1K6ULwhJkgcgiHNm9J1Geqq2azW6PKxQTFjC8rMm5bGxRcYWA

{"blockhash":"11111111111111111111111111111111","signers":["AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi=3uZndChSmPoYfaCihC993E7EAHKDsuu53Ge6Dk1K6ULwhJkgcgiHNm9J1Geqq2azW6PKxQTFjC8rMm5bGxRcYWA"]}
```

### Verify the Signature

Using the _Signers_ output from the [previous step](#create-and-sign-a-dummy-transaction)
to reconstruct the transaction, this time specifying the _pubkey_ and _signature_
as in the submission step of [offline signing](../offline-signing/README.md). That is, the `--from` and
`--fee-payer` are explicitly set to the _pubkey_ rather than being taken from
the keypair (which is not queried this time).

Command

```text
solana transfer 11111111111111111111111111111111 0 --sign-only --from AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi \
--signer AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi=3uZndChSmPoYfaCihC993E7EAHKDsuu53Ge6Dk1K6ULwhJkgcgiHNm9J1Geqq2azW6PKxQTFjC8rMm5bGxRcYWA \
--blockhash 11111111111111111111111111111111 --fee-payer AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi
```

Output

```text
Blockhash: 11111111111111111111111111111111
Signers (Pubkey=Signature):
  AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi=3uZndChSmPoYfaCihC993E7EAHKDsuu53Ge6Dk1K6ULwhJkgcgiHNm9J1Geqq2azW6PKxQTFjC8rMm5bGxRcYWA

{"blockhash":"11111111111111111111111111111111","signers":["AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi=3uZndChSmPoYfaCihC993E7EAHKDsuu53Ge6Dk1K6ULwhJkgcgiHNm9J1Geqq2azW6PKxQTFjC8rMm5bGxRcYWA"]}
```

### An Example of Failure

To simulate an error the [verification step](#verify-the-signature) is repeated,
but with a corrupted _signature_ (the last letter is changed from "A" to "B").

Command

```text
solana transfer 11111111111111111111111111111111 0 --sign-only --from AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi \
--signer AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi=3uZndChSmPoYfaCihC993E7EAHKDsuu53Ge6Dk1K6ULwhJkgcgiHNm9J1Geqq2azW6PKxQTFjC8rMm5bGxRcYWB \
--blockhash 11111111111111111111111111111111 --fee-payer AjTz9EX6vXB6EboKpFm7SwrbDannb6icjvEE632D3rfi
```

Output (Error)

```text
Error: BadParameter("Transaction construction failed, incorrect signature or public key provided")
```

## Checking Account Balance

All that is needed to check an account balance is the public key of an account.
To retrieve public keys securely from a paper wallet, follow the
[Public Key Derivation](#public-key-derivation) instructions on an
[air gapped computer](https://en.wikipedia.org/wiki/Air_gap_\(networking\)).
Public keys can then be typed manually or transferred via a USB stick to a
networked machine.

Next, configure the `solana` CLI tool to connect to a particular cluster:

```bash
solana config set --url <CLUSTER URL> # (i.e. http://devnet.solana.com:8899)
```

Finally, to check the balance, run the following command:

```bash
solana balance <PUBKEY>
```

In order to check a list of public keys quickly, append public keys to a file,
one per line, like so:

`public_keys.txt`
```bash
7hTw3XhprjT2DkVxVixtig9eZwHTZ2rksTSYN7Jh5niZ
9ufAiSyboCZmmEsoStgLYQfnx9KfqP1ZtDK8Wr1j8SJV
# ...
```

And run the following command:
```bash
while read PUBLIC_KEY;
do echo "$PUBLIC_KEY: $(solana balance "$PUBLIC_KEY" | tail -n1)";
done < public_keys.txt
```

## Running a Validator

In order to run a validator, you will need to specify an "identity keypair"
which will be used to fund all of the vote transactions signed by your validator.
Rather than specifying a path with `--identity-keypair <PATH>` you can use the
`--ask-seed-phrase` option.

```bash
solana-validator --ask-seed-phrase identity-keypair --ledger ...

[identity-keypair] seed phrase: 🔒
[identity-keypair] If this seed phrase has an associated passphrase, enter it now. Otherwise, press ENTER to continue:
```

The `--ask-seed-phrase` option accepts multiple keypairs. If you wish to use this
input method for your voting keypair as well you can do the following:

```bash
solana-validator --ask-seed-phrase identity-keypair voting-keypair --ledger ...

[identity-keypair] seed phrase: 🔒
[identity-keypair] If this seed phrase has an associated passphrase, enter it now. Otherwise, press ENTER to continue:
[voting-keypair] seed phrase: 🔒
[voting-keypair] If this seed phrase has an associated passphrase, enter it now. Otherwise, press ENTER to continue:
```

Refer to the following page for a comprehensive guide on running a validator:
{% page-ref page="../running-validator/README.md" %}

## Delegating Stake

Solana CLI tooling supports secure keypair input for stake delegation. To do so,
first create a stake account with some SOL. Use the special `ASK` keyword to
trigger a seed phrase input prompt for the stake account and use
`--keypair ASK` to securely input the funding keypair.

```bash
solana create-stake-account ASK 1 --keypair ASK

[stake_account] seed phrase: 🔒
[stake_account] If this seed phrase has an associated passphrase, enter it now. Otherwise, press ENTER to continue:
[keypair] seed phrase: 🔒
[keypair] If this seed phrase has an associated passphrase, enter it now. Otherwise, press ENTER to continue:
```

Then, to delegate that stake to a validator, use `--keypair ASK` to
securely input the funding keypair.

```bash
solana delegate-stake --keypair ASK <STAKE_ACCOUNT_PUBKEY> <VOTE_ACCOUNT_PUBKEY>

[keypair] seed phrase: 🔒
[keypair] If this seed phrase has an associated passphrase, enter it now. Otherwise, press ENTER to continue:
```

Refer to the following page for a comprehensive guide on delegating stake:
{% page-ref page="../running-validator/validator-stake.md" %}

---

{% page-ref page="../api-reference/cli.md" %}
