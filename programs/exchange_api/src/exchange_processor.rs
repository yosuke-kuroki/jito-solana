//! Config processor

use crate::exchange_instruction::*;
use crate::exchange_state::*;
use crate::id;
use log::*;
use solana_sdk::account::KeyedAccount;
use solana_sdk::instruction::InstructionError;
use solana_sdk::pubkey::Pubkey;
use std::cmp;

pub struct ExchangeProcessor {}

impl ExchangeProcessor {
    #[allow(clippy::needless_pass_by_value)]
    fn map_to_invalid_arg(err: std::boxed::Box<bincode::ErrorKind>) -> InstructionError {
        warn!("Deserialze failed: {:?}", err);
        InstructionError::InvalidArgument
    }

    fn is_account_unallocated(data: &[u8]) -> Result<(), InstructionError> {
        let state: ExchangeState = bincode::deserialize(data).map_err(Self::map_to_invalid_arg)?;
        match state {
            ExchangeState::Unallocated => Ok(()),
            _ => {
                error!("New account is already in use");
                Err(InstructionError::InvalidAccountData)?
            }
        }
    }

    fn deserialize_account(data: &[u8]) -> Result<(TokenAccountInfo), InstructionError> {
        let state: ExchangeState = bincode::deserialize(data).map_err(Self::map_to_invalid_arg)?;
        match state {
            ExchangeState::Account(account) => Ok(account),
            _ => {
                error!("Not a valid account");
                Err(InstructionError::InvalidAccountData)?
            }
        }
    }

    fn deserialize_trade(data: &[u8]) -> Result<(TradeOrderInfo), InstructionError> {
        let state: ExchangeState = bincode::deserialize(data).map_err(Self::map_to_invalid_arg)?;
        match state {
            ExchangeState::Trade(info) => Ok(info),
            _ => {
                error!("Not a valid trade");
                Err(InstructionError::InvalidAccountData)?
            }
        }
    }

    fn serialize(state: &ExchangeState, data: &mut [u8]) -> Result<(), InstructionError> {
        let writer = std::io::BufWriter::new(data);
        match bincode::serialize_into(writer, state) {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("Serialize failed: {:?}", e);
                Err(InstructionError::GenericError)?
            }
        }
    }

    fn calculate_swap(
        scaler: u64,
        swap: &mut TradeSwapInfo,
        to_trade: &mut TradeOrderInfo,
        from_trade: &mut TradeOrderInfo,
        to_trade_account: &mut TokenAccountInfo,
        from_trade_account: &mut TokenAccountInfo,
        profit_account: &mut TokenAccountInfo,
    ) -> Result<(), InstructionError> {
        if to_trade.tokens == 0 || from_trade.tokens == 0 {
            error!("Inactive Trade, balance is zero");
            Err(InstructionError::InvalidArgument)?
        }
        if to_trade.price == 0 || from_trade.price == 0 {
            error!("Inactive Trade, price is zero");
            Err(InstructionError::InvalidArgument)?
        }

        // Calc swap

        trace!("tt {} ft {}", to_trade.tokens, from_trade.tokens);
        trace!("tp {} fp {}", to_trade.price, from_trade.price);

        let max_to_secondary = to_trade.tokens * to_trade.price / scaler;
        let max_to_primary = from_trade.tokens * scaler / from_trade.price;

        trace!("mtp {} mts {}", max_to_primary, max_to_secondary);

        let max_primary = cmp::min(max_to_primary, to_trade.tokens);
        let max_secondary = cmp::min(max_to_secondary, from_trade.tokens);

        trace!("mp {} ms {}", max_primary, max_secondary);

        let primary_tokens = if max_secondary < max_primary {
            max_secondary * scaler / from_trade.price
        } else {
            max_primary
        };
        let secondary_tokens = if max_secondary < max_primary {
            max_secondary
        } else {
            max_primary * to_trade.price / scaler
        };

        if primary_tokens == 0 || secondary_tokens == 0 {
            error!("Trade quantities to low to be fulfilled");
            Err(InstructionError::InvalidArgument)?
        }

        trace!("pt {} st {}", primary_tokens, secondary_tokens);

        let primary_cost = cmp::max(primary_tokens, secondary_tokens * scaler / to_trade.price);
        let secondary_cost = cmp::max(secondary_tokens, primary_tokens * from_trade.price / scaler);

        trace!("pc {} sc {}", primary_cost, secondary_cost);

        let primary_profit = primary_cost - primary_tokens;
        let secondary_profit = secondary_cost - secondary_tokens;

        trace!("pp {} sp {}", primary_profit, secondary_profit);

        let primary_token = to_trade.pair.primary();
        let secondary_token = from_trade.pair.secondary();

        // Update tokens/accounts

        if to_trade.tokens < primary_cost {
            error!("Not enough tokens in to account");
            Err(InstructionError::InvalidArgument)?
        }
        if from_trade.tokens < secondary_cost {
            error!("Not enough tokens in from account");
            Err(InstructionError::InvalidArgument)?
        }
        to_trade.tokens -= primary_cost;
        from_trade.tokens -= secondary_cost;

        to_trade_account.tokens[secondary_token] += secondary_tokens;
        from_trade_account.tokens[primary_token] += primary_tokens;

        profit_account.tokens[primary_token] += primary_profit;
        profit_account.tokens[secondary_token] += secondary_profit;

        swap.pair = to_trade.pair;
        swap.primary_tokens = primary_cost;
        swap.primary_price = to_trade.price;
        swap.secondary_tokens = secondary_cost;
        swap.secondary_price = from_trade.price;

        Ok(())
    }

    fn do_account_request(ka: &mut [KeyedAccount]) -> Result<(), InstructionError> {
        if ka.len() < 2 {
            error!("Not enough accounts");
            Err(InstructionError::InvalidArgument)?
        }

        Self::is_account_unallocated(&ka[1].account.data[..])?;
        Self::serialize(
            &ExchangeState::Account(TokenAccountInfo::default().owner(&ka[0].unsigned_key())),
            &mut ka[1].account.data[..],
        )
    }

    fn do_transfer_request(
        ka: &mut [KeyedAccount],
        token: Token,
        tokens: u64,
    ) -> Result<(), InstructionError> {
        if ka.len() < 3 {
            error!("Not enough accounts");
            Err(InstructionError::InvalidArgument)?
        }

        let mut to_account = Self::deserialize_account(&ka[1].account.data[..])?;

        if &id() == ka[2].unsigned_key() {
            to_account.tokens[token] += tokens;
        } else {
            let mut from_account = Self::deserialize_account(&ka[2].account.data[..])?;

            if &from_account.owner != ka[0].unsigned_key() {
                error!("Signer does not own from account");
                Err(InstructionError::GenericError)?
            }

            if from_account.tokens[token] < tokens {
                error!("From account balance too low");
                Err(InstructionError::GenericError)?
            }

            from_account.tokens[token] -= tokens;
            to_account.tokens[token] += tokens;

            Self::serialize(
                &ExchangeState::Account(from_account),
                &mut ka[1].account.data[..],
            )?;
        }

        Self::serialize(
            &ExchangeState::Account(to_account),
            &mut ka[1].account.data[..],
        )
    }

    fn do_trade_request(
        ka: &mut [KeyedAccount],
        info: TradeRequestInfo,
    ) -> Result<(), InstructionError> {
        if ka.len() < 3 {
            error!("Not enough accounts");
            Err(InstructionError::InvalidArgument)?
        }

        Self::is_account_unallocated(&ka[1].account.data[..])?;

        let mut account = Self::deserialize_account(&ka[2].account.data[..])?;

        if &account.owner != ka[0].unsigned_key() {
            error!("Signer does not own account");
            Err(InstructionError::GenericError)?
        }
        let from_token = match info.direction {
            Direction::To => info.pair.primary(),
            Direction::From => info.pair.secondary(),
        };
        if account.tokens[from_token] < info.tokens {
            error!("From token balance is too low");
            Err(InstructionError::GenericError)?
        }

        if let Err(e) = check_trade(info.direction, info.tokens, info.price) {
            bincode::serialize(&e).unwrap();
        }

        // Trade holds the tokens in escrow
        account.tokens[from_token] -= info.tokens;

        Self::serialize(
            &ExchangeState::Trade(TradeOrderInfo {
                owner: *ka[0].unsigned_key(),
                direction: info.direction,
                pair: info.pair,
                tokens: info.tokens,
                price: info.price,
                src_account: *ka[2].unsigned_key(),
                dst_account: info.dst_account,
            }),
            &mut ka[1].account.data[..],
        )?;
        Self::serialize(
            &ExchangeState::Account(account),
            &mut ka[2].account.data[..],
        )
    }

    fn do_trade_cancellation(ka: &mut [KeyedAccount]) -> Result<(), InstructionError> {
        if ka.len() < 3 {
            error!("Not enough accounts");
            Err(InstructionError::InvalidArgument)?
        }
        let mut trade = Self::deserialize_trade(&ka[1].account.data[..])?;
        let mut account = Self::deserialize_account(&ka[2].account.data[..])?;

        if &trade.owner != ka[0].unsigned_key() {
            error!("Signer does not own trade");
            Err(InstructionError::GenericError)?
        }

        if &account.owner != ka[0].unsigned_key() {
            error!("Signer does not own account");
            Err(InstructionError::GenericError)?
        }

        let token = match trade.direction {
            Direction::To => trade.pair.primary(),
            Direction::From => trade.pair.secondary(),
        };

        // Outstanding tokens transferred back to account
        account.tokens[token] += trade.tokens;
        // Trade becomes invalid
        trade.tokens = 0;

        Self::serialize(&ExchangeState::Trade(trade), &mut ka[1].account.data[..])?;
        Self::serialize(
            &ExchangeState::Account(account),
            &mut ka[2].account.data[..],
        )
    }

    fn do_swap_request(ka: &mut [KeyedAccount]) -> Result<(), InstructionError> {
        if ka.len() < 7 {
            error!("Not enough accounts");
            Err(InstructionError::InvalidArgument)?
        }
        Self::is_account_unallocated(&ka[1].account.data[..])?;
        let mut to_trade = Self::deserialize_trade(&ka[2].account.data[..])?;
        let mut from_trade = Self::deserialize_trade(&ka[3].account.data[..])?;
        let mut to_trade_account = Self::deserialize_account(&ka[4].account.data[..])?;
        let mut from_trade_account = Self::deserialize_account(&ka[5].account.data[..])?;
        let mut profit_account = Self::deserialize_account(&ka[6].account.data[..])?;

        if &to_trade.dst_account != ka[4].unsigned_key() {
            error!("To trade account and to account differ");
            Err(InstructionError::InvalidArgument)?
        }
        if &from_trade.dst_account != ka[5].unsigned_key() {
            error!("From trade account and from account differ");
            Err(InstructionError::InvalidArgument)?
        }
        if to_trade.direction != Direction::To {
            error!("To trade is not a To");
            Err(InstructionError::InvalidArgument)?
        }
        if from_trade.direction != Direction::From {
            error!("From trade is not a From");
            Err(InstructionError::InvalidArgument)?
        }
        if to_trade.pair != from_trade.pair {
            error!("Mismatched token pairs");
            Err(InstructionError::InvalidArgument)?
        }
        if to_trade.direction == from_trade.direction {
            error!("Matching trade directions");
            Err(InstructionError::InvalidArgument)?
        }

        let mut swap = TradeSwapInfo::default();
        swap.to_trade_order = *ka[2].unsigned_key();
        swap.from_trade_order = *ka[3].unsigned_key();

        if let Err(e) = Self::calculate_swap(
            SCALER,
            &mut swap,
            &mut to_trade,
            &mut from_trade,
            &mut to_trade_account,
            &mut from_trade_account,
            &mut profit_account,
        ) {
            error!(
                "Swap calculation failed from {} for {} to {} for {}",
                from_trade.tokens, from_trade.price, to_trade.tokens, to_trade.price,
            );
            Err(e)?
        }

        Self::serialize(&ExchangeState::Swap(swap), &mut ka[1].account.data[..])?;
        Self::serialize(&ExchangeState::Trade(to_trade), &mut ka[2].account.data[..])?;
        Self::serialize(
            &ExchangeState::Trade(from_trade),
            &mut ka[3].account.data[..],
        )?;
        Self::serialize(
            &ExchangeState::Account(to_trade_account),
            &mut ka[4].account.data[..],
        )?;
        Self::serialize(
            &ExchangeState::Account(from_trade_account),
            &mut ka[5].account.data[..],
        )?;
        Self::serialize(
            &ExchangeState::Account(profit_account),
            &mut ka[6].account.data[..],
        )
    }
}

pub fn process_instruction(
    _program_id: &Pubkey,
    keyed_accounts: &mut [KeyedAccount],
    data: &[u8],
    _tick_height: u64,
) -> Result<(), InstructionError> {
    let command = bincode::deserialize::<ExchangeInstruction>(data).map_err(|err| {
        info!("Invalid transaction data: {:?} {:?}", data, err);
        InstructionError::InvalidInstructionData
    })?;

    trace!("{:?}", command);

    match command {
        ExchangeInstruction::AccountRequest => {
            ExchangeProcessor::do_account_request(keyed_accounts)
        }
        ExchangeInstruction::TransferRequest(token, tokens) => {
            ExchangeProcessor::do_transfer_request(keyed_accounts, token, tokens)
        }
        ExchangeInstruction::TradeRequest(info) => {
            ExchangeProcessor::do_trade_request(keyed_accounts, info)
        }
        ExchangeInstruction::TradeCancellation => {
            ExchangeProcessor::do_trade_cancellation(keyed_accounts)
        }
        ExchangeInstruction::SwapRequest => ExchangeProcessor::do_swap_request(keyed_accounts),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use solana_runtime::bank::Bank;
    use solana_runtime::bank_client::BankClient;
    use solana_sdk::genesis_block::GenesisBlock;
    use solana_sdk::signature::{Keypair, KeypairUtil};
    use solana_sdk::system_instruction::SystemInstruction;
    use std::mem;

    fn try_calc(
        scaler: u64,
        primary_tokens: u64,
        primary_price: u64,
        secondary_tokens: u64,
        secondary_price: u64,
        primary_tokens_expect: u64,
        secondary_tokens_expect: u64,
        primary_account_tokens: Tokens,
        secondary_account_tokens: Tokens,
        profit_account_tokens: Tokens,
    ) -> Result<(), InstructionError> {
        trace!(
            "Swap {} for {} to {} for {}",
            primary_tokens,
            primary_price,
            secondary_tokens,
            secondary_price,
        );
        let mut swap = TradeSwapInfo::default();
        let mut to_trade = TradeOrderInfo::default();
        let mut from_trade = TradeOrderInfo::default().direction(Direction::From);
        let mut to_account = TokenAccountInfo::default();
        let mut from_account = TokenAccountInfo::default();
        let mut profit_account = TokenAccountInfo::default();

        to_trade.tokens = primary_tokens;
        to_trade.price = primary_price;
        from_trade.tokens = secondary_tokens;
        from_trade.price = secondary_price;
        ExchangeProcessor::calculate_swap(
            scaler,
            &mut swap,
            &mut to_trade,
            &mut from_trade,
            &mut to_account,
            &mut from_account,
            &mut profit_account,
        )?;

        trace!(
            "{:?} {:?} {:?} {:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
            to_trade.tokens,
            primary_tokens_expect,
            from_trade.tokens,
            secondary_tokens_expect,
            to_account.tokens,
            primary_account_tokens,
            from_account.tokens,
            secondary_account_tokens,
            profit_account.tokens,
            profit_account_tokens
        );

        assert_eq!(to_trade.tokens, primary_tokens_expect);
        assert_eq!(from_trade.tokens, secondary_tokens_expect);
        assert_eq!(to_account.tokens, primary_account_tokens);
        assert_eq!(from_account.tokens, secondary_account_tokens);
        assert_eq!(profit_account.tokens, profit_account_tokens);
        assert_eq!(swap.primary_tokens, primary_tokens - to_trade.tokens);
        assert_eq!(swap.primary_price, to_trade.price);
        assert_eq!(swap.secondary_tokens, secondary_tokens - from_trade.tokens);
        assert_eq!(swap.secondary_price, from_trade.price);
        Ok(())
    }

    #[test]
    #[rustfmt::skip]
    fn test_calculate_swap() {
        solana_logger::setup();

        try_calc(1,     50,     2,   50,    1,  0, 0, Tokens::new(0,  50, 0, 0), Tokens::new(  50, 0, 0, 0), Tokens::new(   0, 0, 0, 0)).unwrap_err();
        try_calc(1,     50,     1,    0,    1,  0, 0, Tokens::new(0,  50, 0, 0), Tokens::new(  50, 0, 0, 0), Tokens::new(   0, 0, 0, 0)).unwrap_err();
        try_calc(1,      0,     1,   50,    1,  0, 0, Tokens::new(0,  50, 0, 0), Tokens::new(  50, 0, 0, 0), Tokens::new(   0, 0, 0, 0)).unwrap_err();
        try_calc(1,     50,     1,   50,    0,  0, 0, Tokens::new(0,  50, 0, 0), Tokens::new(  50, 0, 0, 0), Tokens::new(   0, 0, 0, 0)).unwrap_err();
        try_calc(1,     50,     0,   50,    1,  0, 0, Tokens::new(0,  50, 0, 0), Tokens::new(  50, 0, 0, 0), Tokens::new(   0, 0, 0, 0)).unwrap_err();
        try_calc(1,       1,    2,    2,    3,  1, 2, Tokens::new(0,   0, 0, 0), Tokens::new(   0, 0, 0, 0), Tokens::new(   0, 0, 0, 0)).unwrap_err();

        try_calc(1,     50,     1,   50,    1,  0, 0, Tokens::new(0,  50, 0, 0), Tokens::new(  50, 0, 0, 0), Tokens::new(   0, 0, 0, 0)).unwrap();
        try_calc(1,       1,    2,    3,    3,  0, 0, Tokens::new(0,   2, 0, 0), Tokens::new(   1, 0, 0, 0), Tokens::new(   0, 1, 0, 0)).unwrap();
        try_calc(1,       2,    2,    3,    3,  1, 0, Tokens::new(0,   2, 0, 0), Tokens::new(   1, 0, 0, 0), Tokens::new(   0, 1, 0, 0)).unwrap();
        try_calc(1,       3,    2,    3,    3,  2, 0, Tokens::new(0,   2, 0, 0), Tokens::new(   1, 0, 0, 0), Tokens::new(   0, 1, 0, 0)).unwrap();
        try_calc(1,       3,    2,    6,    3,  1, 0, Tokens::new(0,   4, 0, 0), Tokens::new(   2, 0, 0, 0), Tokens::new(   0, 2, 0, 0)).unwrap();
        try_calc(1000,    1, 2000,    3, 3000,  0, 0, Tokens::new(0,   2, 0, 0), Tokens::new(   1, 0, 0, 0), Tokens::new(   0, 1, 0, 0)).unwrap();
        try_calc(1,       3,    2,    7,    3,  1, 1, Tokens::new(0,   4, 0, 0), Tokens::new(   2, 0, 0, 0), Tokens::new(   0, 2, 0, 0)).unwrap();
        try_calc(1000, 3000,  333, 1000,  500,  0, 1, Tokens::new(0, 999, 0, 0), Tokens::new(1998, 0, 0, 0), Tokens::new(1002, 0, 0, 0)).unwrap();
        try_calc(1000,   50,  100,   50,  101,  0,45, Tokens::new(0,   5, 0, 0), Tokens::new(  49, 0, 0, 0), Tokens::new(   1, 0, 0, 0)).unwrap();
    }

    fn create_bank(lamports: u64) -> (Bank, Keypair) {
        let (genesis_block, mint_keypair) = GenesisBlock::new(lamports);
        let mut bank = Bank::new(&genesis_block);
        bank.add_instruction_processor(id(), process_instruction);
        (bank, mint_keypair)
    }

    fn create_client(bank: &Bank, mint_keypair: Keypair) -> (BankClient, Pubkey) {
        let owner = Keypair::new();
        let pubkey = owner.pubkey();
        let mint_client = BankClient::new(&bank, mint_keypair);
        mint_client
            .process_instruction(SystemInstruction::new_move(
                &mint_client.pubkey(),
                &owner.pubkey(),
                42,
            ))
            .expect("new_move");

        let client = BankClient::new(&bank, owner);

        (client, pubkey)
    }

    fn create_account(client: &BankClient, owner: &Pubkey) -> Pubkey {
        let new = Keypair::new().pubkey();
        let instruction = SystemInstruction::new_program_account(
            &owner,
            &new,
            1,
            mem::size_of::<ExchangeState>() as u64,
            &id(),
        );
        client
            .process_instruction(instruction)
            .expect(&format!("{}:{}", line!(), file!()));
        new
    }

    fn create_token_account(client: &BankClient, owner: &Pubkey) -> Pubkey {
        let new = Keypair::new().pubkey();
        let instruction = SystemInstruction::new_program_account(
            &owner,
            &new,
            1,
            mem::size_of::<ExchangeState>() as u64,
            &id(),
        );
        client
            .process_instruction(instruction)
            .expect(&format!("{}:{}", line!(), file!()));
        let instruction = ExchangeInstruction::new_account_request(&owner, &new);
        client
            .process_instruction(instruction)
            .expect(&format!("{}:{}", line!(), file!()));
        new
    }

    fn transfer(client: &BankClient, owner: &Pubkey, to: &Pubkey, token: Token, tokens: u64) {
        let instruction =
            ExchangeInstruction::new_transfer_request(owner, to, &id(), token, tokens);
        client
            .process_instruction(instruction)
            .expect(&format!("{}:{}", line!(), file!()));
    }

    fn trade(
        client: &BankClient,
        owner: &Pubkey,
        direction: Direction,
        pair: TokenPair,
        from_token: Token,
        src_tokens: u64,
        trade_tokens: u64,
        price: u64,
    ) -> (Pubkey, Pubkey, Pubkey) {
        let trade = create_account(&client, &owner);
        let src = create_token_account(&client, &owner);
        let dst = create_token_account(&client, &owner);
        transfer(&client, &owner, &src, from_token, src_tokens);

        let instruction = ExchangeInstruction::new_trade_request(
            owner,
            &trade,
            direction,
            pair,
            trade_tokens,
            price,
            &src,
            &dst,
        );
        client
            .process_instruction(instruction)
            .expect(&format!("{}:{}", line!(), file!()));
        (trade, src, dst)
    }

    fn deserialize_swap(data: &[u8]) -> TradeSwapInfo {
        let state: ExchangeState =
            bincode::deserialize(data).expect(&format!("{}:{}", line!(), file!()));
        match state {
            ExchangeState::Swap(info) => info,
            _ => panic!("Not a valid swap"),
        }
    }

    #[test]
    fn test_exchange_new_account() {
        solana_logger::setup();
        let (bank, mint_keypair) = create_bank(10_000);
        let (client, owner) = create_client(&bank, mint_keypair);

        let new = create_token_account(&client, &owner);
        let new_account = bank.get_account(&new).unwrap();

        // Check results

        assert_eq!(
            TokenAccountInfo::default().owner(&owner),
            ExchangeProcessor::deserialize_account(&new_account.data[..]).unwrap()
        );
    }

    #[test]
    fn test_exchange_new_account_not_unallocated() {
        solana_logger::setup();
        let (bank, mint_keypair) = create_bank(10_000);
        let (client, owner) = create_client(&bank, mint_keypair);

        let new = create_token_account(&client, &owner);
        let instruction = ExchangeInstruction::new_account_request(&owner, &new);
        client
            .process_instruction(instruction)
            .expect_err(&format!("{}:{}", line!(), file!()));
    }

    #[test]
    fn test_exchange_new_transfer_request() {
        solana_logger::setup();
        let (bank, mint_keypair) = create_bank(10_000);
        let (client, owner) = create_client(&bank, mint_keypair);

        let new = create_token_account(&client, &owner);

        let instruction =
            ExchangeInstruction::new_transfer_request(&owner, &new, &id(), Token::A, 42);
        client
            .process_instruction(instruction)
            .expect(&format!("{}:{}", line!(), file!()));

        let new_account = bank.get_account(&new).unwrap();

        // Check results

        assert_eq!(
            TokenAccountInfo::default()
                .owner(&owner)
                .tokens(42, 0, 0, 0),
            ExchangeProcessor::deserialize_account(&new_account.data[..]).unwrap()
        );
    }

    #[test]
    fn test_exchange_new_trade_request() {
        solana_logger::setup();
        let (bank, mint_keypair) = create_bank(10_000);
        let (client, owner) = create_client(&bank, mint_keypair);

        let (trade, src, dst) = trade(
            &client,
            &owner,
            Direction::To,
            TokenPair::AB,
            Token::A,
            42,
            2,
            1000,
        );

        let trade_account = bank.get_account(&trade).unwrap();
        let src_account = bank.get_account(&src).unwrap();
        let dst_account = bank.get_account(&dst).unwrap();

        // check results

        assert_eq!(
            TradeOrderInfo {
                owner: owner,
                direction: Direction::To,
                pair: TokenPair::AB,
                tokens: 2,
                price: 1000,
                src_account: src,
                dst_account: dst
            },
            ExchangeProcessor::deserialize_trade(&trade_account.data[..]).unwrap()
        );
        assert_eq!(
            TokenAccountInfo::default()
                .owner(&owner)
                .tokens(40, 0, 0, 0),
            ExchangeProcessor::deserialize_account(&src_account.data[..]).unwrap()
        );
        assert_eq!(
            TokenAccountInfo::default().owner(&owner).tokens(0, 0, 0, 0),
            ExchangeProcessor::deserialize_account(&dst_account.data[..]).unwrap()
        );
    }

    #[test]
    fn test_exchange_new_swap_request() {
        solana_logger::setup();
        let (bank, mint_keypair) = create_bank(10_000);
        let (client, owner) = create_client(&bank, mint_keypair);

        let swap = create_account(&client, &owner);
        let profit = create_token_account(&client, &owner);
        let (to_trade, to_src, to_dst) = trade(
            &client,
            &owner,
            Direction::To,
            TokenPair::AB,
            Token::A,
            2,
            2,
            2000,
        );
        let (from_trade, from_src, from_dst) = trade(
            &client,
            &owner,
            Direction::From,
            TokenPair::AB,
            Token::B,
            3,
            3,
            3000,
        );

        let instruction = ExchangeInstruction::new_swap_request(
            &owner,
            &swap,
            &to_trade,
            &from_trade,
            &to_dst,
            &from_dst,
            &profit,
        );
        client
            .process_instruction(instruction)
            .expect(&format!("{}:{}", line!(), file!()));

        let to_trade_account = bank.get_account(&to_trade).unwrap();
        let to_src_account = bank.get_account(&to_src).unwrap();
        let to_dst_account = bank.get_account(&to_dst).unwrap();
        let from_trade_account = bank.get_account(&from_trade).unwrap();
        let from_src_account = bank.get_account(&from_src).unwrap();
        let from_dst_account = bank.get_account(&from_dst).unwrap();
        let profit_account = bank.get_account(&profit).unwrap();
        let swap_account = bank.get_account(&swap).unwrap();

        // check results

        assert_eq!(
            TradeOrderInfo {
                owner: owner,
                direction: Direction::To,
                pair: TokenPair::AB,
                tokens: 1,
                price: 2000,
                src_account: to_src,
                dst_account: to_dst
            },
            ExchangeProcessor::deserialize_trade(&to_trade_account.data[..]).unwrap()
        );
        assert_eq!(
            TokenAccountInfo::default().owner(&owner).tokens(0, 0, 0, 0),
            ExchangeProcessor::deserialize_account(&to_src_account.data[..]).unwrap()
        );
        assert_eq!(
            TokenAccountInfo::default().owner(&owner).tokens(0, 2, 0, 0),
            ExchangeProcessor::deserialize_account(&to_dst_account.data[..]).unwrap()
        );
        assert_eq!(
            TradeOrderInfo {
                owner: owner,
                direction: Direction::From,
                pair: TokenPair::AB,
                tokens: 0,
                price: 3000,
                src_account: from_src,
                dst_account: from_dst
            },
            ExchangeProcessor::deserialize_trade(&from_trade_account.data[..]).unwrap()
        );
        assert_eq!(
            TokenAccountInfo::default().owner(&owner).tokens(0, 0, 0, 0),
            ExchangeProcessor::deserialize_account(&from_src_account.data[..]).unwrap()
        );
        assert_eq!(
            TokenAccountInfo::default().owner(&owner).tokens(1, 0, 0, 0),
            ExchangeProcessor::deserialize_account(&from_dst_account.data[..]).unwrap()
        );
        assert_eq!(
            TokenAccountInfo::default().owner(&owner).tokens(0, 1, 0, 0),
            ExchangeProcessor::deserialize_account(&profit_account.data[..]).unwrap()
        );
        assert_eq!(
            TradeSwapInfo {
                pair: TokenPair::AB,
                to_trade_order: to_trade,
                from_trade_order: from_trade,
                primary_tokens: 1,
                primary_price: 2000,
                secondary_tokens: 3,
                secondary_price: 3000,
            },
            deserialize_swap(&swap_account.data[..])
        );
    }
}
