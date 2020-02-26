use crate::{
    input_parsers::{derivation_of, pubkeys_sigs_of},
    offline::SIGNER_ARG,
    ArgConstant,
};
use bip39::{Language, Mnemonic, Seed};
use clap::{values_t, ArgMatches, Error, ErrorKind};
use rpassword::prompt_password_stderr;
use solana_remote_wallet::{
    remote_keypair::generate_remote_keypair,
    remote_wallet::{RemoteWalletError, RemoteWalletManager},
};
use solana_sdk::{
    pubkey::Pubkey,
    signature::{
        keypair_from_seed, keypair_from_seed_phrase_and_passphrase, read_keypair,
        read_keypair_file, Keypair, Presigner, Signature, Signer,
    },
};
use std::{
    error,
    io::{stdin, stdout, Write},
    process::exit,
    str::FromStr,
    sync::Arc,
};

pub enum KeypairUrl {
    Ask,
    Filepath(String),
    Usb(String),
    Stdin,
    Pubkey(Pubkey),
}

pub fn parse_keypair_path(path: &str) -> KeypairUrl {
    if path == "-" {
        KeypairUrl::Stdin
    } else if path == ASK_KEYWORD {
        KeypairUrl::Ask
    } else if path.starts_with("usb://") {
        KeypairUrl::Usb(path.to_string())
    } else if let Ok(pubkey) = Pubkey::from_str(path) {
        KeypairUrl::Pubkey(pubkey)
    } else {
        KeypairUrl::Filepath(path.to_string())
    }
}

pub fn presigner_from_pubkey_sigs(
    pubkey: &Pubkey,
    signers: &[(Pubkey, Signature)],
) -> Option<Presigner> {
    signers.iter().find_map(|(signer, sig)| {
        if *signer == *pubkey {
            Some(Presigner::new(signer, sig))
        } else {
            None
        }
    })
}

pub fn signer_from_path(
    matches: &ArgMatches,
    path: &str,
    keypair_name: &str,
    wallet_manager: Option<&Arc<RemoteWalletManager>>,
) -> Result<Box<dyn Signer>, Box<dyn error::Error>> {
    match parse_keypair_path(path) {
        KeypairUrl::Ask => {
            let skip_validation = matches.is_present(SKIP_SEED_PHRASE_VALIDATION_ARG.name);
            Ok(Box::new(keypair_from_seed_phrase(
                keypair_name,
                skip_validation,
                false,
            )?))
        }
        KeypairUrl::Filepath(path) => Ok(Box::new(read_keypair_file(&path)?)),
        KeypairUrl::Stdin => {
            let mut stdin = std::io::stdin();
            Ok(Box::new(read_keypair(&mut stdin)?))
        }
        KeypairUrl::Usb(path) => {
            if let Some(wallet_manager) = wallet_manager {
                Ok(Box::new(generate_remote_keypair(
                    path,
                    derivation_of(matches, "derivation_path"),
                    wallet_manager,
                    matches.is_present("confirm_key"),
                )?))
            } else {
                Err(RemoteWalletError::NoDeviceFound.into())
            }
        }
        KeypairUrl::Pubkey(pubkey) => {
            let presigner = pubkeys_sigs_of(matches, SIGNER_ARG.name)
                .as_ref()
                .and_then(|presigners| presigner_from_pubkey_sigs(&pubkey, presigners));
            if let Some(presigner) = presigner {
                Ok(Box::new(presigner))
            } else {
                Err(Error::with_description(
                    "Missing signature for supplied pubkey",
                    ErrorKind::MissingRequiredArgument,
                )
                .into())
            }
        }
    }
}

// Keyword used to indicate that the user should be asked for a keypair seed phrase
pub const ASK_KEYWORD: &str = "ASK";

pub const ASK_SEED_PHRASE_ARG: ArgConstant<'static> = ArgConstant {
    long: "ask-seed-phrase",
    name: "ask_seed_phrase",
    help: "Recover a keypair using a seed phrase and optional passphrase",
};

pub const SKIP_SEED_PHRASE_VALIDATION_ARG: ArgConstant<'static> = ArgConstant {
    long: "skip-seed-phrase-validation",
    name: "skip_seed_phrase_validation",
    help: "Skip validation of seed phrases. Use this if your phrase does not use the BIP39 official English word list",
};

#[derive(Debug, PartialEq)]
pub enum Source {
    Generated,
    Path,
    SeedPhrase,
}

pub struct KeypairWithSource {
    pub keypair: Keypair,
    pub source: Source,
}

impl KeypairWithSource {
    fn new(keypair: Keypair, source: Source) -> Self {
        Self { keypair, source }
    }
}

/// Prompts user for a passphrase and then asks for confirmirmation to check for mistakes
pub fn prompt_passphrase(prompt: &str) -> Result<String, Box<dyn error::Error>> {
    let passphrase = prompt_password_stderr(&prompt)?;
    if !passphrase.is_empty() {
        let confirmed = rpassword::prompt_password_stderr("Enter same passphrase again: ")?;
        if confirmed != passphrase {
            return Err("Passphrases did not match".into());
        }
    }
    Ok(passphrase)
}

/// Reads user input from stdin to retrieve a seed phrase and passphrase for keypair derivation
/// Optionally skips validation of seed phrase
/// Optionally confirms recovered public key
pub fn keypair_from_seed_phrase(
    keypair_name: &str,
    skip_validation: bool,
    confirm_pubkey: bool,
) -> Result<Keypair, Box<dyn error::Error>> {
    let seed_phrase = prompt_password_stderr(&format!("[{}] seed phrase: ", keypair_name))?;
    let seed_phrase = seed_phrase.trim();
    let passphrase_prompt = format!(
        "[{}] If this seed phrase has an associated passphrase, enter it now. Otherwise, press ENTER to continue: ",
        keypair_name,
    );

    let keypair = if skip_validation {
        let passphrase = prompt_passphrase(&passphrase_prompt)?;
        keypair_from_seed_phrase_and_passphrase(&seed_phrase, &passphrase)?
    } else {
        let sanitized = sanitize_seed_phrase(seed_phrase);
        let mnemonic = Mnemonic::from_phrase(&sanitized, Language::English)?;
        let passphrase = prompt_passphrase(&passphrase_prompt)?;
        let seed = Seed::new(&mnemonic, &passphrase);
        keypair_from_seed(seed.as_bytes())?
    };

    if confirm_pubkey {
        let pubkey = keypair.pubkey();
        print!("Recovered pubkey `{:?}`. Continue? (y/n): ", pubkey);
        let _ignored = stdout().flush();
        let mut input = String::new();
        stdin().read_line(&mut input).expect("Unexpected input");
        if input.to_lowercase().trim() != "y" {
            println!("Exiting");
            exit(1);
        }
    }

    Ok(keypair)
}

/// Checks CLI arguments to determine whether a keypair should be:
///   - inputted securely via stdin,
///   - read in from a file,
///   - or newly generated
pub fn keypair_input(
    matches: &clap::ArgMatches,
    keypair_name: &str,
) -> Result<KeypairWithSource, Box<dyn error::Error>> {
    let ask_seed_phrase_matches =
        values_t!(matches.values_of(ASK_SEED_PHRASE_ARG.name), String).unwrap_or_default();
    let keypair_match_name = keypair_name.replace('-', "_");
    if ask_seed_phrase_matches
        .iter()
        .any(|s| s.as_str() == keypair_name)
    {
        if matches.value_of(keypair_match_name).is_some() {
            clap::Error::with_description(
                &format!(
                    "`--{} {}` cannot be used with `{} <PATH>`",
                    ASK_SEED_PHRASE_ARG.long, keypair_name, keypair_name
                ),
                clap::ErrorKind::ArgumentConflict,
            )
            .exit();
        }

        let skip_validation = matches.is_present(SKIP_SEED_PHRASE_VALIDATION_ARG.name);
        keypair_from_seed_phrase(keypair_name, skip_validation, true)
            .map(|keypair| KeypairWithSource::new(keypair, Source::SeedPhrase))
    } else if let Some(keypair_file) = matches.value_of(keypair_match_name) {
        if keypair_file.starts_with("usb://") {
            Ok(KeypairWithSource::new(Keypair::new(), Source::Path))
        } else {
            read_keypair_file(keypair_file)
                .map(|keypair| KeypairWithSource::new(keypair, Source::Path))
        }
    } else {
        Ok(KeypairWithSource::new(Keypair::new(), Source::Generated))
    }
}

fn sanitize_seed_phrase(seed_phrase: &str) -> String {
    seed_phrase
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ArgMatches;

    #[test]
    fn test_keypair_input() {
        let arg_matches = ArgMatches::default();
        let KeypairWithSource { source, .. } = keypair_input(&arg_matches, "").unwrap();
        assert_eq!(source, Source::Generated);
    }

    #[test]
    fn test_sanitize_seed_phrase() {
        let seed_phrase = " Mary   had\ta\u{2009}little  \n\t lamb";
        assert_eq!(
            "Mary had a little lamb".to_owned(),
            sanitize_seed_phrase(seed_phrase)
        );
    }
}
