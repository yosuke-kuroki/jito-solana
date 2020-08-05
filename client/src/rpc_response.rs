use crate::client_error;
use solana_account_decoder::UiAccount;
use solana_sdk::{
    clock::{Epoch, Slot},
    fee_calculator::{FeeCalculator, FeeRateGovernor},
    inflation::Inflation,
    transaction::{Result, TransactionError},
};
use solana_transaction_status::ConfirmedTransactionStatusWithSignature;
use std::{collections::HashMap, net::SocketAddr};

pub type RpcResult<T> = client_error::Result<Response<T>>;
pub type RpcAmount = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponseContext {
    pub slot: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response<T> {
    pub context: RpcResponseContext,
    pub value: T,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcBlockCommitment<T> {
    pub commitment: Option<T>,
    pub total_stake: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcBlockhashFeeCalculator {
    pub blockhash: String,
    pub fee_calculator: FeeCalculator,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcFees {
    pub blockhash: String,
    pub fee_calculator: FeeCalculator,
    pub last_valid_slot: Slot,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcFeeCalculator {
    pub fee_calculator: FeeCalculator,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcFeeRateGovernor {
    pub fee_rate_governor: FeeRateGovernor,
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcInflationGovernor {
    pub initial: f64,
    pub terminal: f64,
    pub taper: f64,
    pub foundation: f64,
    pub foundation_term: f64,
}

impl From<Inflation> for RpcInflationGovernor {
    fn from(inflation: Inflation) -> Self {
        Self {
            initial: inflation.initial,
            terminal: inflation.terminal,
            taper: inflation.taper,
            foundation: inflation.foundation,
            foundation_term: inflation.foundation_term,
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcInflationRate {
    pub total: f64,
    pub validator: f64,
    pub foundation: f64,
    pub epoch: Epoch,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcKeyedAccount {
    pub pubkey: String,
    pub account: UiAccount,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcSignatureResult {
    pub err: Option<TransactionError>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RpcContactInfo {
    /// Pubkey of the node as a base-58 string
    pub pubkey: String,
    /// Gossip port
    pub gossip: Option<SocketAddr>,
    /// Tpu port
    pub tpu: Option<SocketAddr>,
    /// JSON RPC port
    pub rpc: Option<SocketAddr>,
    /// Software version
    pub version: Option<String>,
}

/// Map of leader base58 identity pubkeys to the slot indices relative to the first epoch slot
pub type RpcLeaderSchedule = HashMap<String, Vec<usize>>;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct RpcVersionInfo {
    /// The current version of solana-core
    pub solana_core: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct RpcIdentity {
    /// The current node identity pubkey
    pub identity: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcVoteAccountStatus {
    pub current: Vec<RpcVoteAccountInfo>,
    pub delinquent: Vec<RpcVoteAccountInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcVoteAccountInfo {
    /// Vote account pubkey as base-58 encoded string
    pub vote_pubkey: String,

    /// The pubkey of the node that votes using this account
    pub node_pubkey: String,

    /// The current stake, in lamports, delegated to this vote account
    pub activated_stake: u64,

    /// An 8-bit integer used as a fraction (commission/MAX_U8) for rewards payout
    pub commission: u8,

    /// Whether this account is staked for the current epoch
    pub epoch_vote_account: bool,

    /// History of how many credits earned by the end of each epoch
    ///   each tuple is (Epoch, credits, prev_credits)
    pub epoch_credits: Vec<(Epoch, u64, u64)>,

    /// Most recent slot voted on by this vote account (0 if no votes exist)
    pub last_vote: u64,

    /// Current root slot for this vote account (0 if not root slot exists)
    pub root_slot: Slot,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcSignatureConfirmation {
    pub confirmations: usize,
    pub status: Result<()>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcSimulateTransactionResult {
    pub err: Option<TransactionError>,
    pub logs: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcStorageTurn {
    pub blockhash: String,
    pub slot: Slot,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RpcAccountBalance {
    pub address: String,
    pub lamports: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcSupply {
    pub total: u64,
    pub circulating: u64,
    pub non_circulating: u64,
    pub non_circulating_accounts: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub enum StakeActivationState {
    Activating,
    Active,
    Deactivating,
    Inactive,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcStakeActivation {
    pub state: StakeActivationState,
    pub active: u64,
    pub inactive: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RpcTokenAmount {
    pub ui_amount: f64,
    pub decimals: u8,
    pub amount: RpcAmount,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RpcTokenAccountBalance {
    pub address: String,
    #[serde(flatten)]
    pub amount: RpcTokenAmount,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcConfirmedTransactionStatusWithSignature {
    pub signature: String,
    pub slot: Slot,
    pub err: Option<TransactionError>,
    pub memo: Option<String>,
}

impl From<ConfirmedTransactionStatusWithSignature> for RpcConfirmedTransactionStatusWithSignature {
    fn from(value: ConfirmedTransactionStatusWithSignature) -> Self {
        let ConfirmedTransactionStatusWithSignature {
            signature,
            slot,
            err,
            memo,
        } = value;
        Self {
            signature: signature.to_string(),
            slot,
            err,
            memo,
        }
    }
}
