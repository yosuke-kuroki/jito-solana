//! A command-line executable for generating the chain's genesis block.

use clap::{crate_description, crate_name, crate_version, value_t_or_exit, App, Arg};
use solana::blocktree::create_new_ledger;
use solana_sdk::genesis_block::GenesisBlock;
use solana_sdk::signature::{read_keypair, Keypair, KeypairUtil};
use std::error;

/**
 * Bootstrap leader gets two lamports:
 * - one lamport to use as stake
 * - one lamport to keep the node identity public key valid
 */
pub const BOOTSTRAP_LEADER_LAMPORTS: u64 = 2;

fn main() -> Result<(), Box<dyn error::Error>> {
    let matches = App::new(crate_name!())
        .about(crate_description!())
        .version(crate_version!())
        .arg(
            Arg::with_name("bootstrap_leader_keypair_file")
                .short("b")
                .long("bootstrap-leader-keypair")
                .value_name("BOOTSTRAP LEADER KEYPAIR")
                .takes_value(true)
                .required(true)
                .help("Path to file containing the bootstrap leader's keypair"),
        )
        .arg(
            Arg::with_name("ledger_path")
                .short("l")
                .long("ledger")
                .value_name("DIR")
                .takes_value(true)
                .required(true)
                .help("Use directory as persistent ledger location"),
        )
        .arg(
            Arg::with_name("lamports")
                .short("t")
                .long("lamports")
                .value_name("LAMPORTS")
                .takes_value(true)
                .required(true)
                .help("Number of lamports to create in the mint"),
        )
        .arg(
            Arg::with_name("mint_keypair_file")
                .short("m")
                .long("mint")
                .value_name("MINT")
                .takes_value(true)
                .required(true)
                .help("Path to file containing keys of the mint"),
        )
        .get_matches();

    let bootstrap_leader_keypair_file = matches.value_of("bootstrap_leader_keypair_file").unwrap();
    let ledger_path = matches.value_of("ledger_path").unwrap();
    let mint_keypair_file = matches.value_of("mint_keypair_file").unwrap();
    let lamports = value_t_or_exit!(matches, "lamports", u64);

    let bootstrap_leader_keypair = read_keypair(bootstrap_leader_keypair_file)?;
    let mint_keypair = read_keypair(mint_keypair_file)?;

    let bootstrap_leader_vote_account_keypair = Keypair::new();
    let (mut genesis_block, _mint_keypair) = GenesisBlock::new_with_leader(
        lamports,
        &bootstrap_leader_keypair.pubkey(),
        BOOTSTRAP_LEADER_LAMPORTS,
    );
    genesis_block.mint_id = mint_keypair.pubkey();
    genesis_block.bootstrap_leader_vote_account_id = bootstrap_leader_vote_account_keypair.pubkey();
    genesis_block.native_programs.extend_from_slice(&[
        ("solana_budget_program".to_string(), solana_budget_api::id()),
        (
            "solana_storage_program".to_string(),
            solana_storage_api::id(),
        ),
        ("solana_token_program".to_string(), solana_token_api::id()),
        ("solana_config_program".to_string(), solana_config_api::id()),
    ]);

    create_new_ledger(ledger_path, &genesis_block)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use hashbrown::HashSet;
    use solana_sdk::pubkey::Pubkey;

    #[test]
    fn test_program_ids() {
        let system = Pubkey::new(&[
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let native = Pubkey::new(&[
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let bpf = Pubkey::new(&[
            128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let budget = Pubkey::new(&[
            129, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let storage = Pubkey::new(&[
            130, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let token = Pubkey::new(&[
            131, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let vote = Pubkey::new(&[
            132, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);
        let config = Pubkey::new(&[
            133, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ]);

        assert_eq!(solana_sdk::system_program::id(), system);
        assert_eq!(solana_sdk::native_loader::id(), native);
        assert_eq!(solana_sdk::bpf_loader::id(), bpf);
        assert_eq!(solana_budget_api::id(), budget);
        assert_eq!(solana_storage_api::id(), storage);
        assert_eq!(solana_token_api::id(), token);
        assert_eq!(solana_vote_api::id(), vote);
        assert_eq!(solana_config_api::id(), config);
    }

    #[test]
    fn test_program_id_uniqueness() {
        let mut unique = HashSet::new();
        let ids = vec![
            solana_sdk::system_program::id(),
            solana_sdk::native_loader::id(),
            solana_sdk::bpf_loader::id(),
            solana_budget_api::id(),
            solana_storage_api::id(),
            solana_token_api::id(),
            solana_vote_api::id(),
            solana_config_api::id(),
        ];
        assert!(ids.into_iter().all(move |id| unique.insert(id)));
    }
}
