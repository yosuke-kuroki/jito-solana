# solana CLI

The [solana-cli crate](https://crates.io/crates/solana-cli) provides a command-line interface tool for Solana

## Examples

### Get Pubkey

```bash
// Command
$ solana address

// Return
<PUBKEY>
```

### Airdrop SOL/Lamports

```bash
// Command
$ solana airdrop 2

// Return
"2.00000000 SOL"
```

### Get Balance

```bash
// Command
$ solana balance

// Return
"3.00050001 SOL"
```

### Confirm Transaction

```bash
// Command
$ solana confirm <TX_SIGNATURE>

// Return
"Confirmed" / "Not found" / "Transaction failed with error <ERR>"
```

### Deploy program

```bash
// Command
$ solana deploy <PATH>

// Return
<PROGRAM_ID>
```

### Unconditional Immediate Transfer

```bash
// Command
$ solana pay <PUBKEY> 123

// Return
<TX_SIGNATURE>
```

### Post-Dated Transfer

```bash
// Command
$ solana pay <PUBKEY> 123 \
    --after 2018-12-24T23:59:00 --require-timestamp-from <PUBKEY>

// Return
{signature: <TX_SIGNATURE>, processId: <PROCESS_ID>}
```

_`require-timestamp-from` is optional. If not provided, the transaction will expect a timestamp signed by this wallet's private key_

### Authorized Transfer

A third party must send a signature to unlock the lamports.

```bash
// Command
$ solana pay <PUBKEY> 123 \
    --require-signature-from <PUBKEY>

// Return
{signature: <TX_SIGNATURE>, processId: <PROCESS_ID>}
```

### Post-Dated and Authorized Transfer

```bash
// Command
$ solana pay <PUBKEY> 123 \
    --after 2018-12-24T23:59 --require-timestamp-from <PUBKEY> \
    --require-signature-from <PUBKEY>

// Return
{signature: <TX_SIGNATURE>, processId: <PROCESS_ID>}
```

### Multiple Witnesses

```bash
// Command
$ solana pay <PUBKEY> 123 \
    --require-signature-from <PUBKEY> \
    --require-signature-from <PUBKEY>

// Return
{signature: <TX_SIGNATURE>, processId: <PROCESS_ID>}
```

### Cancelable Transfer

```bash
// Command
$ solana pay <PUBKEY> 123 \
    --require-signature-from <PUBKEY> \
    --cancelable

// Return
{signature: <TX_SIGNATURE>, processId: <PROCESS_ID>}
```

### Cancel Transfer

```bash
// Command
$ solana cancel <PROCESS_ID>

// Return
<TX_SIGNATURE>
```

### Send Signature

```bash
// Command
$ solana send-signature <PUBKEY> <PROCESS_ID>

// Return
<TX_SIGNATURE>
```

### Indicate Elapsed Time

Use the current system time:

```bash
// Command
$ solana send-timestamp <PUBKEY> <PROCESS_ID>

// Return
<TX_SIGNATURE>
```

Or specify some other arbitrary timestamp:

```bash
// Command
$ solana send-timestamp <PUBKEY> <PROCESS_ID> --date 2018-12-24T23:59:00

// Return
<TX_SIGNATURE>
```

## Usage
### solana-cli
```text
solana-cli 1.1.0 [channel=unknown commit=unknown]
Blockchain, Rebuilt for Scale

USAGE:
    solana [FLAGS] [OPTIONS] <SUBCOMMAND>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

SUBCOMMANDS:
    account                             Show the contents of an account
    address                             Get your public key
    airdrop                             Request lamports
    authorize-nonce-account             Assign account authority to a new entity
    balance                             Get your balance
    block-production                    Show information about block production
    block-time                          Get estimated production time of a block
    cancel                              Cancel a transfer
    catchup                             Wait for a validator to catch up to the cluster
    claim-storage-reward                Redeem storage reward credits
    cluster-version                     Get the version of the cluster entrypoint
    config                              Solana command-line tool configuration settings
    confirm                             Confirm transaction by signature
    create-address-with-seed            Generate a derived account address with a seed
    create-archiver-storage-account     Create an archiver storage account
    create-nonce-account                Create a nonce account
    create-stake-account                Create a stake account
    create-validator-storage-account    Create a validator storage account
    create-vote-account                 Create a vote account
    deactivate-stake                    Deactivate the delegated stake from the stake account
    delegate-stake                      Delegate stake to a vote account
    deploy                              Deploy a program
    epoch-info                          Get information about the current epoch
    fees                                Display current cluster fees
    genesis-hash                        Get the genesis hash
    gossip                              Show the current gossip network nodes
    help                                Prints this message or the help of the given subcommand(s)
    leader-schedule                     Display leader schedule
    live-slots                          Show information about the current slot progression
    new-nonce                           Generate a new nonce, rendering the existing nonce useless
    nonce                               Get the current nonce value
    nonce-account                       Show the contents of a nonce account
    pay                                 Send a payment
    ping                                Submit transactions sequentially
    send-signature                      Send a signature to authorize a transfer
    send-timestamp                      Send a timestamp to unlock a transfer
    slot                                Get current slot
    split-stake                         Split a stake account
    stake-account                       Show the contents of a stake account
    stake-authorize-staker              Authorize a new stake signing keypair for the given stake account
    stake-authorize-withdrawer          Authorize a new withdraw signing keypair for the given stake account
    stake-history                       Show the stake history
    stake-set-lockup                    Set Lockup for the stake account
    stakes                              Show stake account information
    storage-account                     Show the contents of a storage account
    transaction-count                   Get current transaction count
    transfer                            Transfer funds between system accounts
    validator-info                      Publish/get Validator info on Solana
    validators                          Show summary information about the current validators
    vote-account                        Show the contents of a vote account
    vote-authorize-voter                Authorize a new vote signing keypair for the given vote account
    vote-authorize-withdrawer           Authorize a new withdraw signing keypair for the given vote account
    vote-update-validator               Update the vote account's validator identity
    withdraw-from-nonce-account         Withdraw lamports from the nonce account
    withdraw-stake                      Withdraw the unstaked lamports from the stake account
```

#### solana-account
```text
solana-account 
Show the contents of an account

USAGE:
    solana account [FLAGS] [OPTIONS] <ACCOUNT PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --lamports                       Display balance in lamports instead of SOL
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
    -o, --output <FILE>                                  Write the account data to this file

ARGS:
    <ACCOUNT PUBKEY>    Account pubkey
```

#### solana-address
```text
solana-address 
Get your public key

USAGE:
    solana address [FLAGS] [OPTIONS]

FLAGS:
        --confirm-key                    Confirm key on device; only relevant if using remote wallet
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-airdrop
```text
solana-airdrop 
Request lamports

USAGE:
    solana airdrop [FLAGS] [OPTIONS] <AMOUNT> [PUBKEY]

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --faucet-host <HOST>                             Faucet host to use [default: the --url host]
        --faucet-port <PORT>                             Faucet port to use [default: 9900]
    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <AMOUNT>    The airdrop amount to request, in SOL
    <PUBKEY>    The pubkey of airdrop recipient
```

#### solana-authorize-nonce-account
```text
solana-authorize-nonce-account 
Assign account authority to a new entity

USAGE:
    solana authorize-nonce-account [FLAGS] [OPTIONS] <NONCE_ACCOUNT> <NEW_AUTHORITY_PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --seed <SEED STRING>
            Seed for address generation; if specified, the resulting account will be at a derived address of the
            NONCE_ACCOUNT pubkey

ARGS:
    <NONCE_ACCOUNT>           Address of the nonce account
    <NEW_AUTHORITY_PUBKEY>    Account to be granted authority of the nonce account
```

#### solana-balance
```text
solana-balance 
Get your balance

USAGE:
    solana balance [FLAGS] [OPTIONS] [PUBKEY]

FLAGS:
    -h, --help                           Prints help information
        --lamports                       Display balance in lamports instead of SOL
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <PUBKEY>    The public key of the balance to check
```

#### solana-block-production
```text
solana-block-production 
Show information about block production

USAGE:
    solana block-production [FLAGS] [OPTIONS]

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --epoch <epoch>                                  Epoch to show block production for [default: current epoch]
    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
        --slot-limit <slot_limit>
            Limit results to this many slots from the end of the epoch [default: full epoch]
```

#### solana-block-time
```text
solana-block-time 
Get estimated production time of a block

USAGE:
    solana block-time [FLAGS] [OPTIONS] <SLOT>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <SLOT>    Slot number of the block to query
```

#### solana-cancel
```text
solana-cancel 
Cancel a transfer

USAGE:
    solana cancel [FLAGS] [OPTIONS] <PROCESS ID>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <PROCESS ID>    The process id of the transfer to cancel
```

#### solana-catchup
```text
solana-catchup 
Wait for a validator to catch up to the cluster

USAGE:
    solana catchup [FLAGS] [OPTIONS] <PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <PUBKEY>    Identity pubkey of the validator
```

#### solana-claim-storage-reward
```text
solana-claim-storage-reward 
Redeem storage reward credits

USAGE:
    solana claim-storage-reward [FLAGS] [OPTIONS] <NODE PUBKEY> <STORAGE ACCOUNT PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <NODE PUBKEY>               The node account to credit the rewards to
    <STORAGE ACCOUNT PUBKEY>    Storage account address to redeem credits for
```

#### solana-cluster-version
```text
solana-cluster-version 
Get the version of the cluster entrypoint

USAGE:
    solana cluster-version [FLAGS] [OPTIONS]

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-config
```text
solana-config 
Solana command-line tool configuration settings

USAGE:
    solana config [FLAGS] [OPTIONS] <SUBCOMMAND>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

SUBCOMMANDS:
    get     Get current config settings
    help    Prints this message or the help of the given subcommand(s)
    set     Set a config setting
```

#### solana-confirm
```text
solana-confirm 
Confirm transaction by signature

USAGE:
    solana confirm [FLAGS] [OPTIONS] <SIGNATURE>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <SIGNATURE>    The transaction signature to confirm
```

#### solana-create-address-with-seed
```text
solana-create-address-with-seed 
Generate a derived account address with a seed

USAGE:
    solana create-address-with-seed [FLAGS] [OPTIONS] <SEED_STRING> <PROGRAM_ID>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --from <PUBKEY>                                  From (base) key, defaults to client keypair.
    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <SEED_STRING>    The seed.  Must not take more than 32 bytes to encode as utf-8
    <PROGRAM_ID>     The program_id that the address will ultimately be used for, 
                     or one of STAKE, VOTE, and STORAGE keywords
```

#### solana-create-archiver-storage-account
```text
solana-create-archiver-storage-account 
Create an archiver storage account

USAGE:
    solana create-archiver-storage-account [FLAGS] [OPTIONS] <STORAGE ACCOUNT OWNER PUBKEY> <STORAGE ACCOUNT>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <STORAGE ACCOUNT OWNER PUBKEY>    
    <STORAGE ACCOUNT>                 
```

#### solana-create-nonce-account
```text
solana-create-nonce-account 
Create a nonce account

USAGE:
    solana create-nonce-account [FLAGS] [OPTIONS] <NONCE ACCOUNT> <AMOUNT>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
        --nonce-authority <BASE58_PUBKEY>                Assign noncing authority to another entity

ARGS:
    <NONCE ACCOUNT>    Keypair of the nonce account to fund
    <AMOUNT>           The amount to load the nonce account with, in SOL
```

#### solana-create-stake-account
```text
solana-create-stake-account 
Create a stake account

USAGE:
    solana create-stake-account [FLAGS] [OPTIONS] <STAKE ACCOUNT> <AMOUNT>

FLAGS:
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                        Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --custodian <KEYPAIR or PUBKEY>
            Identity of the custodian (can withdraw before lockup expires)

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --fee-payer <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Specify the fee-payer account. This may be a keypair file, the ASK keyword 
            or the pubkey of an offline signer, provided an appropriate --signer argument 
            is also passed. Defaults to the client keypair.
        --from <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Source account of funds (if different from client local account)

    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --lockup-date <RFC3339 DATE TIME>
            The date and time at which this account will be available for withdrawal

        --lockup-epoch <EPOCH>
            The epoch height at which this account will be available for withdrawal

        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --seed <SEED STRING>
            Seed for address generation; if specified, the resulting account will be at a derived address of the STAKE
            ACCOUNT pubkey
        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction

        --stake-authority <PUBKEY>
            Public key of authorized staker (defaults to cli config pubkey)

        --withdraw-authority <PUBKEY>
            Public key of authorized withdrawer (defaults to cli config pubkey)


ARGS:
    <STAKE ACCOUNT>    Signing authority of the stake address to fund
    <AMOUNT>           The amount of send to the vote account, in SOL
```

#### solana-create-validator-storage-account
```text
solana-create-validator-storage-account 
Create a validator storage account

USAGE:
    solana create-validator-storage-account [FLAGS] [OPTIONS] <STORAGE ACCOUNT OWNER PUBKEY> <STORAGE ACCOUNT>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <STORAGE ACCOUNT OWNER PUBKEY>    
    <STORAGE ACCOUNT>                 
```

#### solana-create-vote-account
```text
solana-create-vote-account 
Create a vote account

USAGE:
    solana create-vote-account [FLAGS] [OPTIONS] <VOTE ACCOUNT KEYPAIR> <VALIDATOR IDENTITY PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --authorized-voter <PUBKEY>                      Public key of the authorized voter (defaults to vote account)
        --authorized-withdrawer <PUBKEY>
            Public key of the authorized withdrawer (defaults to cli config pubkey)

        --commission <NUM>
            The commission taken on reward redemption (0-100) [default: 100]

    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
        --seed <SEED STRING>
            Seed for address generation; if specified, the resulting account will be at a derived address of the VOTE
            ACCOUNT pubkey

ARGS:
    <VOTE ACCOUNT KEYPAIR>         Vote account keypair to fund
    <VALIDATOR IDENTITY PUBKEY>    Validator that will vote with this account
```

#### solana-deactivate-stake
```text
solana-deactivate-stake 
Deactivate the delegated stake from the stake account

USAGE:
    solana deactivate-stake [FLAGS] [OPTIONS] <STAKE ACCOUNT>

FLAGS:
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                        Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --fee-payer <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Specify the fee-payer account. This may be a keypair file, the ASK keyword 
            or the pubkey of an offline signer, provided an appropriate --signer argument 
            is also passed. Defaults to the client keypair.
    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction

        --stake-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Public key of authorized staker (defaults to cli config pubkey)


ARGS:
    <STAKE ACCOUNT>    Stake account to be deactivated.
```

#### solana-delegate-stake
```text
solana-delegate-stake 
Delegate stake to a vote account

USAGE:
    solana delegate-stake [FLAGS] [OPTIONS] <STAKE ACCOUNT> <VOTE ACCOUNT>

FLAGS:
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                        Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --fee-payer <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Specify the fee-payer account. This may be a keypair file, the ASK keyword 
            or the pubkey of an offline signer, provided an appropriate --signer argument 
            is also passed. Defaults to the client keypair.
    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction

        --stake-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Public key of authorized staker (defaults to cli config pubkey)


ARGS:
    <STAKE ACCOUNT>    Stake account to delegate
    <VOTE ACCOUNT>     The vote account to which the stake will be delegated
```

#### solana-deploy
```text
solana-deploy 
Deploy a program

USAGE:
    solana deploy [FLAGS] [OPTIONS] <PATH TO BPF PROGRAM>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <PATH TO BPF PROGRAM>    /path/to/program.o
```

#### solana-epoch-info
```text
solana-epoch-info 
Get information about the current epoch

USAGE:
    solana epoch-info [FLAGS] [OPTIONS]

FLAGS:
        --confirmed                      Return information at maximum-lockout commitment level
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-fees
```text
solana-fees 
Display current cluster fees

USAGE:
    solana fees [FLAGS] [OPTIONS]

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-genesis-hash
```text
solana-genesis-hash 
Get the genesis hash

USAGE:
    solana genesis-hash [FLAGS] [OPTIONS]

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-gossip
```text
solana-gossip 
Show the current gossip network nodes

USAGE:
    solana gossip [FLAGS] [OPTIONS]

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-help
```text
solana-help 
Prints this message or the help of the given subcommand(s)

USAGE:
    solana help [subcommand]...

ARGS:
    <subcommand>...    The subcommand whose help message to display
```

#### solana-leader-schedule
```text
solana-leader-schedule 
Display leader schedule

USAGE:
    solana leader-schedule [FLAGS] [OPTIONS]

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-live-slots
```text
solana-live-slots 
Show information about the current slot progression

USAGE:
    solana live-slots [FLAGS] [OPTIONS]

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
    -w, --ws <URL>
            WebSocket URL for PubSub RPC connection [default: ws://127.0.0.1:8900]
```

#### solana-new-nonce
```text
solana-new-nonce 
Generate a new nonce, rendering the existing nonce useless

USAGE:
    solana new-nonce [FLAGS] [OPTIONS] <NONCE ACCOUNT>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction


ARGS:
    <NONCE ACCOUNT>    Address of the nonce account
```

#### solana-nonce
```text
solana-nonce 
Get the current nonce value

USAGE:
    solana nonce [FLAGS] [OPTIONS] <NONCE ACCOUNT>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <NONCE ACCOUNT>    Address of the nonce account to display
```

#### solana-nonce-account
```text
solana-nonce-account 
Show the contents of a nonce account

USAGE:
    solana nonce-account [FLAGS] [OPTIONS] <NONCE ACCOUNT>

FLAGS:
    -h, --help                           Prints help information
        --lamports                       Display balance in lamports instead of SOL
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <NONCE ACCOUNT>    Address of the nonce account to display
```

#### solana-pay
```text
solana-pay 
Send a payment

USAGE:
    solana pay [FLAGS] [OPTIONS] <TO PUBKEY> <AMOUNT>

FLAGS:
        --cancelable                     
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                        Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction

        --after <DATETIME>                                             A timestamp after which transaction will execute
        --require-timestamp-from <PUBKEY>                              Require timestamp from this third party
        --require-signature-from <PUBKEY>...
            Any third party signatures required to unlock the lamports


ARGS:
    <TO PUBKEY>    The pubkey of recipient
    <AMOUNT>       The amount to send, in SOL
```

#### solana-ping
```text
solana-ping 
Submit transactions sequentially

USAGE:
    solana ping [FLAGS] [OPTIONS]

FLAGS:
        --confirmed                      Wait until the transaction is confirmed at maximum-lockout commitment level
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

    -c, --count <NUMBER>                                 Stop after submitting count transactions
        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -i, --interval <SECONDS>
            Wait interval seconds between submitting the next transaction [default: 2]

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
        --lamports <NUMBER>
            Number of lamports to transfer for each transaction [default: 1]

    -t, --timeout <SECONDS>
            Wait up to timeout seconds for transaction confirmation [default: 15]
```

#### solana-send-signature
```text
solana-send-signature 
Send a signature to authorize a transfer

USAGE:
    solana send-signature [FLAGS] [OPTIONS] <PUBKEY> <PROCESS ID>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <PUBKEY>        The pubkey of recipient
    <PROCESS ID>    The process id of the transfer to authorize
```

#### solana-send-timestamp
```text
solana-send-timestamp 
Send a timestamp to unlock a transfer

USAGE:
    solana send-timestamp [FLAGS] [OPTIONS] <PUBKEY> <PROCESS ID>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --date <DATETIME>                                Optional arbitrary timestamp to apply
        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <PUBKEY>        The pubkey of recipient
    <PROCESS ID>    The process id of the transfer to unlock
```

#### solana-slot
```text
solana-slot 
Get current slot

USAGE:
    solana slot [FLAGS] [OPTIONS]

FLAGS:
        --confirmed                      Return slot at maximum-lockout commitment level
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-split-stake
```text
solana-split-stake 
Split a stake account

USAGE:
    solana split-stake [FLAGS] [OPTIONS] <STAKE ACCOUNT> <SPLIT STAKE ACCOUNT> <AMOUNT>

FLAGS:
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                        Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --fee-payer <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Specify the fee-payer account. This may be a keypair file, the ASK keyword 
            or the pubkey of an offline signer, provided an appropriate --signer argument 
            is also passed. Defaults to the client keypair.
    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --seed <SEED STRING>
            Seed for address generation; if specified, the resulting account will be at a derived address of the SPLIT
            STAKE ACCOUNT pubkey
        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction

        --stake-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Public key of authorized staker (defaults to cli config pubkey)


ARGS:
    <STAKE ACCOUNT>          Stake account to be split
    <SPLIT STAKE ACCOUNT>    Keypair of the new stake account to split funds into
    <AMOUNT>                 The amount to move into the new stake account, in unit SOL
```

#### solana-stake-account
```text
solana-stake-account 
Show the contents of a stake account

USAGE:
    solana stake-account [FLAGS] [OPTIONS] <STAKE ACCOUNT>

FLAGS:
    -h, --help                           Prints help information
        --lamports                       Display balance in lamports instead of SOL
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <STAKE ACCOUNT>    Address of the stake account to display
```

#### solana-stake-authorize-staker
```text
solana-stake-authorize-staker 
Authorize a new stake signing keypair for the given stake account

USAGE:
    solana stake-authorize-staker [FLAGS] [OPTIONS] <STAKE ACCOUNT> <AUTHORIZE PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                        Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --fee-payer <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Specify the fee-payer account. This may be a keypair file, the ASK keyword 
            or the pubkey of an offline signer, provided an appropriate --signer argument 
            is also passed. Defaults to the client keypair.
    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction

        --stake-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Public key of authorized staker (defaults to cli config pubkey)


ARGS:
    <STAKE ACCOUNT>       Stake account in which to set the authorized staker
    <AUTHORIZE PUBKEY>    New authorized staker
```

#### solana-stake-authorize-withdrawer
```text
solana-stake-authorize-withdrawer 
Authorize a new withdraw signing keypair for the given stake account

USAGE:
    solana stake-authorize-withdrawer [FLAGS] [OPTIONS] <STAKE ACCOUNT> <AUTHORIZE PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                           Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --fee-payer <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Specify the fee-payer account. This may be a keypair file, the ASK keyword 
            or the pubkey of an offline signer, provided an appropriate --signer argument 
            is also passed. Defaults to the client keypair.
    -u, --url <URL>                                                       JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                                  /path/to/id.json or usb://remote/wallet/path
        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction

        --withdraw-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Public key of authorized withdrawer (defaults to cli config pubkey)


ARGS:
    <STAKE ACCOUNT>       Stake account in which to set the authorized withdrawer
    <AUTHORIZE PUBKEY>    New authorized withdrawer
```

#### solana-stake-history
```text
solana-stake-history 
Show the stake history

USAGE:
    solana stake-history [FLAGS] [OPTIONS]

FLAGS:
    -h, --help                           Prints help information
        --lamports                       Display balance in lamports instead of SOL
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-stake-set-lockup
```text
solana-stake-set-lockup 
Set Lockup for the stake account

USAGE:
    solana stake-set-lockup [FLAGS] [OPTIONS] <STAKE ACCOUNT>

FLAGS:
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                        Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --custodian <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Public key of signing custodian (defaults to cli config pubkey)

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --fee-payer <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Specify the fee-payer account. This may be a keypair file, the ASK keyword 
            or the pubkey of an offline signer, provided an appropriate --signer argument 
            is also passed. Defaults to the client keypair.
    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --lockup-date <RFC3339 DATE TIME>
            The date and time at which this account will be available for withdrawal

        --lockup-epoch <EPOCH>
            The epoch height at which this account will be available for withdrawal

        --new-custodian <KEYPAIR or PUBKEY>
            Identity of the new lockup custodian (can withdraw before lockup expires)

        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction


ARGS:
    <STAKE ACCOUNT>    Stake account for which to set Lockup
```

#### solana-stakes
```text
solana-stakes 
Show stake account information

USAGE:
    solana stakes [FLAGS] [OPTIONS] [VOTE ACCOUNT PUBKEYS]...

FLAGS:
    -h, --help                           Prints help information
        --lamports                       Display balance in lamports instead of SOL
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <VOTE ACCOUNT PUBKEYS>...    Only show stake accounts delegated to the provided vote accounts
```

#### solana-storage-account
```text
solana-storage-account 
Show the contents of a storage account

USAGE:
    solana storage-account [FLAGS] [OPTIONS] <STORAGE ACCOUNT PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <STORAGE ACCOUNT PUBKEY>    Storage account pubkey
```

#### solana-transaction-count
```text
solana-transaction-count 
Get current transaction count

USAGE:
    solana transaction-count [FLAGS] [OPTIONS]

FLAGS:
        --confirmed                      Return count at maximum-lockout commitment level
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-transfer
```text
solana-transfer 
Transfer funds between system accounts

USAGE:
    solana transfer [FLAGS] [OPTIONS] <TO PUBKEY> <AMOUNT>

FLAGS:
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                        Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --fee-payer <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Specify the fee-payer account. This may be a keypair file, the ASK keyword 
            or the pubkey of an offline signer, provided an appropriate --signer argument 
            is also passed. Defaults to the client keypair.
        --from <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Source account of funds (if different from client local account)

    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction


ARGS:
    <TO PUBKEY>    The pubkey of recipient
    <AMOUNT>       The amount to send, in SOL
```

#### solana-validator-info
```text
solana-validator-info 
Publish/get Validator info on Solana

USAGE:
    solana validator-info [FLAGS] [OPTIONS] <SUBCOMMAND>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

SUBCOMMANDS:
    get        Get and parse Solana Validator info
    help       Prints this message or the help of the given subcommand(s)
    publish    Publish Validator info on Solana
```

#### solana-validators
```text
solana-validators 
Show summary information about the current validators

USAGE:
    solana validators [FLAGS] [OPTIONS]

FLAGS:
    -h, --help                           Prints help information
        --lamports                       Display balance in lamports instead of SOL
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path
```

#### solana-vote-account
```text
solana-vote-account 
Show the contents of a vote account

USAGE:
    solana vote-account [FLAGS] [OPTIONS] <VOTE ACCOUNT PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --lamports                       Display balance in lamports instead of SOL
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <VOTE ACCOUNT PUBKEY>    Vote account pubkey
```

#### solana-vote-authorize-voter
```text
solana-vote-authorize-voter 
Authorize a new vote signing keypair for the given vote account

USAGE:
    solana vote-authorize-voter [FLAGS] [OPTIONS] <VOTE ACCOUNT PUBKEY> <NEW VOTER PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <VOTE ACCOUNT PUBKEY>    Vote account in which to set the authorized voter
    <NEW VOTER PUBKEY>       New vote signer to authorize
```

#### solana-vote-authorize-withdrawer
```text
solana-vote-authorize-withdrawer 
Authorize a new withdraw signing keypair for the given vote account

USAGE:
    solana vote-authorize-withdrawer [FLAGS] [OPTIONS] <VOTE ACCOUNT PUBKEY> <NEW WITHDRAWER PUBKEY>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <VOTE ACCOUNT PUBKEY>      Vote account in which to set the authorized withdrawer
    <NEW WITHDRAWER PUBKEY>    New withdrawer to authorize
```

#### solana-vote-update-validator
```text
solana-vote-update-validator 
Update the vote account's validator identity

USAGE:
    solana vote-update-validator [FLAGS] [OPTIONS] <VOTE ACCOUNT PUBKEY> <NEW VALIDATOR IDENTITY PUBKEY> <AUTHORIZED VOTER KEYPAIR>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                      JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                 /path/to/id.json or usb://remote/wallet/path

ARGS:
    <VOTE ACCOUNT PUBKEY>              Vote account to update
    <NEW VALIDATOR IDENTITY PUBKEY>    New validator that will vote with this account
    <AUTHORIZED VOTER KEYPAIR>         Authorized voter keypair
```

#### solana-withdraw-from-nonce-account
```text
solana-withdraw-from-nonce-account 
Withdraw lamports from the nonce account

USAGE:
    solana withdraw-from-nonce-account [FLAGS] [OPTIONS] <NONCE ACCOUNT> <DESTINATION ACCOUNT> <AMOUNT>

FLAGS:
    -h, --help                           Prints help information
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

    -u, --url <URL>                                                    JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                               /path/to/id.json or usb://remote/wallet/path
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction


ARGS:
    <NONCE ACCOUNT>          Nonce account from to withdraw from
    <DESTINATION ACCOUNT>    The account to which the lamports should be transferred
    <AMOUNT>                 The amount to withdraw from the nonce account, in SOL
```

#### solana-withdraw-stake
```text
solana-withdraw-stake 
Withdraw the unstaked lamports from the stake account

USAGE:
    solana withdraw-stake [FLAGS] [OPTIONS] <STAKE ACCOUNT> <DESTINATION ACCOUNT> <AMOUNT>

FLAGS:
    -h, --help                           Prints help information
        --sign-only                      Sign the transaction offline
        --skip-seed-phrase-validation    Skip validation of seed phrases. Use this if your phrase does not use the BIP39
                                         official English word list
    -V, --version                        Prints version information
    -v, --verbose                        Show extra information header

OPTIONS:
        --blockhash <BLOCKHASH>                                           Use the supplied blockhash
    -C, --config <PATH>
            Configuration file to use [default: ~/.config/solana/cli/config.yml]

        --derivation-path <ACCOUNT or ACCOUNT/CHANGE>
            Derivation path to use: m/44'/501'/ACCOUNT'/CHANGE'; default key is device base pubkey: m/44'/501'/0'

        --fee-payer <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Specify the fee-payer account. This may be a keypair file, the ASK keyword 
            or the pubkey of an offline signer, provided an appropriate --signer argument 
            is also passed. Defaults to the client keypair.
    -u, --url <URL>                                                       JSON RPC URL for the solana cluster
    -k, --keypair <PATH>                                                  /path/to/id.json or usb://remote/wallet/path
        --nonce <PUBKEY>
            Provide the nonce account to use when creating a nonced 
            transaction. Nonced transactions are useful when a transaction 
            requires a lengthy signing process. Learn more about nonced 
            transactions at https://docs.solana.com/offline-signing/durable-nonce
        --nonce-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Provide the nonce authority keypair to use when signing a nonced transaction

        --signer <BASE58_PUBKEY=BASE58_SIG>...
            Provide a public-key/signature pair for the transaction

        --withdraw-authority <KEYPAIR or PUBKEY or REMOTE WALLET PATH>
            Public key of authorized withdrawer (defaults to cli config pubkey)


ARGS:
    <STAKE ACCOUNT>          Stake account from which to withdraw
    <DESTINATION ACCOUNT>    The account to which the lamports should be transferred
    <AMOUNT>                 The amount to withdraw from the stake account, in SOL
```

