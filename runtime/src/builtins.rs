use crate::{
    bank::{Builtin, Builtins},
    system_instruction_processor,
};
use solana_sdk::{
    feature_set,
    instruction::InstructionError,
    keyed_account::KeyedAccount,
    process_instruction::{stable_log, InvokeContext, ProcessInstructionWithContext},
    pubkey::Pubkey,
    system_program,
};

fn process_instruction_with_program_logging(
    process_instruction: ProcessInstructionWithContext,
    program_id: &Pubkey,
    keyed_accounts: &[KeyedAccount],
    instruction_data: &[u8],
    invoke_context: &mut dyn InvokeContext,
) -> Result<(), InstructionError> {
    let logger = invoke_context.get_logger();
    stable_log::program_invoke(&logger, program_id, invoke_context.invoke_depth());

    let result = process_instruction(program_id, keyed_accounts, instruction_data, invoke_context);

    match &result {
        Ok(()) => stable_log::program_success(&logger, program_id),
        Err(err) => stable_log::program_failure(&logger, program_id, err),
    }
    result
}

macro_rules! with_program_logging {
    ($process_instruction:expr) => {
        |program_id: &Pubkey,
         keyed_accounts: &[KeyedAccount],
         instruction_data: &[u8],
         invoke_context: &mut dyn InvokeContext| {
            process_instruction_with_program_logging(
                $process_instruction,
                program_id,
                keyed_accounts,
                instruction_data,
                invoke_context,
            )
        }
    };
}

/// Builtin programs that are always available
fn genesis_builtins() -> Vec<Builtin> {
    vec![
        Builtin::new(
            "system_program",
            system_program::id(),
            with_program_logging!(system_instruction_processor::process_instruction),
        ),
        Builtin::new(
            "vote_program",
            solana_vote_program::id(),
            with_program_logging!(solana_vote_program::vote_instruction::process_instruction),
        ),
        // Remove legacy_stake_processor and move stake_instruction::process_instruction back to
        // genesis_builtins around the v1.6 timeframe
        Builtin::new(
            "stake_program",
            solana_stake_program::id(),
            with_program_logging!(
                solana_stake_program::legacy_stake_processor::process_instruction
            ),
        ),
        Builtin::new(
            "config_program",
            solana_config_program::id(),
            with_program_logging!(solana_config_program::config_processor::process_instruction),
        ),
    ]
}

#[derive(AbiExample, Debug, Clone)]
pub enum ActivationType {
    NewProgram,
    NewVersion,
}

/// Builtin programs activated dynamically by feature
///
/// Note: If the feature_builtin is intended to replace another builtin program, it must have a new
/// name.
/// This is to enable the runtime to determine categorically whether the builtin update has
/// occurred, and preserve idempotency in Bank::add_native_program across genesis, snapshot, and
/// normal child Bank creation.
/// https://github.com/solana-labs/solana/blob/84b139cc94b5be7c9e0c18c2ad91743231b85a0d/runtime/src/bank.rs#L1723
fn feature_builtins() -> Vec<(Builtin, Pubkey, ActivationType)> {
    vec![
        (
            Builtin::new(
                "secp256k1_program",
                solana_sdk::secp256k1_program::id(),
                solana_secp256k1_program::process_instruction,
            ),
            feature_set::secp256k1_program_enabled::id(),
            ActivationType::NewProgram,
        ),
        (
            Builtin::new(
                "stake_program_v3",
                solana_stake_program::id(),
                solana_stake_program::stake_instruction::process_instruction,
            ),
            feature_set::stake_program_v3::id(),
            ActivationType::NewVersion,
        ),
    ]
}

pub(crate) fn get() -> Builtins {
    Builtins {
        genesis_builtins: genesis_builtins(),
        feature_builtins: feature_builtins(),
    }
}
