// @flow

import assert from 'assert';
import {parse as urlParse, format as urlFormat} from 'url';
import fetch from 'node-fetch';
import jayson from 'jayson/lib/client/browser';
import {struct} from 'superstruct';
import {Client as RpcWebSocketClient} from 'rpc-websockets';

import {NonceAccount} from './nonce-account';
import {PublicKey} from './publickey';
import {DEFAULT_TICKS_PER_SLOT, NUM_TICKS_PER_SECOND} from './timing';
import {Transaction} from './transaction';
import {sleep} from './util/sleep';
import type {Blockhash} from './blockhash';
import type {FeeCalculator} from './fee-calculator';
import type {Account} from './account';
import type {TransactionSignature} from './transaction';

type RpcRequest = (methodName: string, args: Array<any>) => any;

type RpcResponseAndContext<T> = {
  context: {
    slot: number,
  },
  value: T,
};

/**
 * @private
 */
function jsonRpcResultAndContext(resultDescription: any) {
  return jsonRpcResult({
    context: struct({
      slot: 'number',
    }),
    value: resultDescription,
  });
}

/**
 * @private
 */
function jsonRpcResult(resultDescription: any) {
  const jsonRpcVersion = struct.literal('2.0');
  return struct.union([
    struct({
      jsonrpc: jsonRpcVersion,
      id: 'string',
      error: 'any',
    }),
    struct({
      jsonrpc: jsonRpcVersion,
      id: 'string',
      error: 'null?',
      result: resultDescription,
    }),
  ]);
}

/**
 * The level of commitment desired when querying state
 *   'max':    Query the most recent block which has reached max voter lockout
 *   'recent': Query the most recent block
 *
 * @typedef {'max' | 'recent'} Commitment
 */
export type Commitment = 'max' | 'recent';

/**
 * Information describing a cluster node
 *
 * @typedef {Object} ContactInfo
 * @property {string} pubkey Identity public key of the node
 * @property {string} gossip Gossip network address for the node
 * @property {string} tpu TPU network address for the node (null if not available)
 * @property {string|null} rpc JSON RPC network address for the node (null if not available)
 */
type ContactInfo = {
  pubkey: string,
  gossip: string,
  tpu: string | null,
  rpc: string | null,
};

/**
 * Information describing a vote account
 *
 * @typedef {Object} VoteAccountInfo
 * @property {string} votePubkey Public key of the vote account
 * @property {string} nodePubkey Identity public key of the node voting with this account
 * @property {number} activatedStake The stake, in lamports, delegated to this vote account and activated
 * @property {boolean} epochVoteAccount Whether the vote account is staked for this epoch
 * @property {Array<Array<number>>} epochCredits Recent epoch voting credit history for this voter
 * @property {number} commission A percentage (0-100) of rewards payout owed to the voter
 * @property {number} lastVote Most recent slot voted on by this vote account
 */
type VoteAccountInfo = {
  votePubkey: string,
  nodePubkey: string,
  activatedStake: number,
  epochVoteAccount: boolean,
  epochCredits: Array<[number, number, number]>,
  commission: number,
  lastVote: number,
};

/**
 * A collection of cluster vote accounts
 *
 * @typedef {Object} VoteAccountStatus
 * @property {Array<VoteAccountInfo>} current Active vote accounts
 * @property {Array<VoteAccountInfo>} delinquent Inactive vote accounts
 */
type VoteAccountStatus = {
  current: Array<VoteAccountInfo>,
  delinquent: Array<VoteAccountInfo>,
};

/**
 * Network Inflation parameters
 * (see https://docs.solana.com/book/v/master/implemented-proposals/ed_overview)
 *
 * @typedef {Object} Inflation
 * @property {number} foundation
 * @property {number} foundation_term
 * @property {number} initial
 * @property {number} storage
 * @property {number} taper
 * @property {number} terminal
 */
const GetInflationResult = struct({
  foundation: 'number',
  foundation_term: 'number',
  initial: 'number',
  storage: 'number',
  taper: 'number',
  terminal: 'number',
});

/**
 * EpochInfo parameters
 * (see https://docs.solana.com/book/v/master/terminology#epoch)
 *
 * @typedef {Object} EpochInfo
 * @property {number} epoch
 * @property {number} slotIndex
 * @property {number} slotsInEpoch
 * @property {number} absoluteSlot
 */
const GetEpochInfoResult = struct({
  epoch: 'number',
  slotIndex: 'number',
  slotsInEpoch: 'number',
  absoluteSlot: 'number',
});

/**
 * EpochSchedule parameters
 * (see https://docs.solana.com/book/v/master/terminology#epoch)
 *
 * @typedef {Object} EpochSchedule
 * @property {number} slots_per_epoch
 * @property {number} leader_schedule_slot_offset
 * @property {boolean} warmup
 * @property {number} first_normal_epoch
 * @property {number} first_normal_slot
 */
const GetEpochScheduleResult = struct({
  slots_per_epoch: 'number',
  leader_schedule_slot_offset: 'number',
  warmup: 'boolean',
  first_normal_epoch: 'number',
  first_normal_slot: 'number',
});

/**
 * Version info for a node
 *
 * @typedef {Object} Version
 * @property {string} solana-core Version of solana-core
 */
const Version = struct({
  'solana-core': 'string',
});

/**
 * A ConfirmedBlock on the ledger
 *
 * @typedef {Object} ConfirmedBlock
 * @property {Blockhash} blockhash Blockhash of this block
 * @property {Blockhash} previousBlockhash Blockhash of this block's parent
 * @property {number} parentSlot Slot index of this block's parent
 * @property {Array<Array<object>>} transactions Vector of transactions paired with statuses
 */
type ConfirmedBlock = {
  blockhash: Blockhash,
  previousBlockhash: Blockhash,
  parentSlot: number,
  transactions: Array<[Transaction, GetSignatureStatusRpcResult]>,
};

function createRpcRequest(url): RpcRequest {
  const server = jayson(async (request, callback) => {
    const options = {
      method: 'POST',
      body: request,
      headers: {
        'Content-Type': 'application/json',
      },
    };

    try {
      const res = await fetch(url, options);
      const text = await res.text();
      callback(null, text);
    } catch (err) {
      callback(err);
    }
  });

  return (method, args) => {
    return new Promise((resolve, reject) => {
      server.request(method, args, (err, response) => {
        if (err) {
          reject(err);
          return;
        }
        resolve(response);
      });
    });
  };
}

/**
 * Expected JSON RPC response for the "getInflation" message
 */
const GetInflationRpcResult = struct({
  jsonrpc: struct.literal('2.0'),
  id: 'string',
  error: 'any?',
  result: GetInflationResult,
});

/**
 * Expected JSON RPC response for the "getEpochInfo" message
 */
const GetEpochInfoRpcResult = struct({
  jsonrpc: struct.literal('2.0'),
  id: 'string',
  error: 'any?',
  result: GetEpochInfoResult,
});

/**
 * Expected JSON RPC response for the "getEpochSchedule" message
 */
const GetEpochScheduleRpcResult = struct({
  jsonrpc: struct.literal('2.0'),
  id: 'string',
  error: 'any?',
  result: GetEpochScheduleResult,
});

/**
 * Expected JSON RPC response for the "getBalance" message
 */
const GetBalanceAndContextRpcResult = jsonRpcResultAndContext('number?');

/**
 * Expected JSON RPC response for the "getVersion" message
 */
const GetVersionRpcResult = struct({
  jsonrpc: struct.literal('2.0'),
  id: 'string',
  error: 'any?',
  result: Version,
});

/**
 * @private
 */
const AccountInfoResult = struct({
  executable: 'boolean',
  owner: 'array',
  lamports: 'number',
  data: 'array',
  rent_epoch: 'number?',
});

/**
 * Expected JSON RPC response for the "getAccountInfo" message
 */
const GetAccountInfoAndContextRpcResult = jsonRpcResultAndContext(
  struct.union(['null', AccountInfoResult]),
);

/***
 * Expected JSON RPC response for the "accountNotification" message
 */
const AccountNotificationResult = struct({
  subscription: 'number',
  result: AccountInfoResult,
});

/**
 * @private
 */
const ProgramAccountInfoResult = struct(['string', AccountInfoResult]);

/***
 * Expected JSON RPC response for the "programNotification" message
 */
const ProgramAccountNotificationResult = struct({
  subscription: 'number',
  result: ProgramAccountInfoResult,
});

/**
 * @private
 */
const SlotInfo = struct({
  parent: 'number',
  slot: 'number',
  root: 'number',
});

/***
 * Expected JSON RPC response for the "slotNotification" message
 */
const SlotNotificationResult = struct({
  subscription: 'number',
  result: SlotInfo,
});

/**
 * Expected JSON RPC response for the "getProgramAccounts" message
 */
const GetProgramAccountsRpcResult = jsonRpcResult(
  struct.list([ProgramAccountInfoResult]),
);

/**
 * Expected JSON RPC response for the "confirmTransaction" message
 */
const ConfirmTransactionAndContextRpcResult = jsonRpcResultAndContext(
  'boolean',
);

/**
 * Expected JSON RPC response for the "getSlot" message
 */
const GetSlot = jsonRpcResult('number');

/**
 * Expected JSON RPC response for the "getSlotLeader" message
 */
const GetSlotLeader = jsonRpcResult('string');

/**
 * Expected JSON RPC response for the "getClusterNodes" message
 */
const GetClusterNodes = jsonRpcResult(
  struct.list([
    struct({
      pubkey: 'string',
      gossip: 'string',
      tpu: struct.union(['null', 'string']),
      rpc: struct.union(['null', 'string']),
    }),
  ]),
);

/**
 * Expected JSON RPC response for the "getVoteAccounts" message
 */
const GetVoteAccounts = jsonRpcResult(
  struct({
    current: struct.list([
      struct({
        votePubkey: 'string',
        nodePubkey: 'string',
        activatedStake: 'number',
        epochVoteAccount: 'boolean',
        epochCredits: struct.list([
          struct.tuple(['number', 'number', 'number']),
        ]),
        commission: 'number',
        lastVote: 'number',
        rootSlot: 'number?',
      }),
    ]),
    delinquent: struct.list([
      struct({
        votePubkey: 'string',
        nodePubkey: 'string',
        activatedStake: 'number',
        epochVoteAccount: 'boolean',
        epochCredits: struct.list([
          struct.tuple(['number', 'number', 'number']),
        ]),
        commission: 'number',
        lastVote: 'number',
        rootSlot: 'number?',
      }),
    ]),
  }),
);

const SignatureStatusResult = struct.union([
  'null',
  struct.union([struct({Ok: 'null'}), struct({Err: 'object'})]),
]);

/**
 * Expected JSON RPC response for the "getSignatureStatus" message
 */
const GetSignatureStatusRpcResult = jsonRpcResult(SignatureStatusResult);

/**
 * Expected JSON RPC response for the "getTransactionCount" message
 */
const GetTransactionCountRpcResult = jsonRpcResult('number');

/**
 * Expected JSON RPC response for the "getTotalSupply" message
 */
const GetTotalSupplyRpcResult = jsonRpcResult('number');

/**
 * Expected JSON RPC response for the "getMinimumBalanceForRentExemption" message
 */
const GetMinimumBalanceForRentExemptionRpcResult = jsonRpcResult('number');

/**
 * Expected JSON RPC response for the "getConfirmedBlock" message
 */
export const GetConfirmedBlockRpcResult = jsonRpcResult(
  struct({
    blockhash: 'string',
    previousBlockhash: 'string',
    parentSlot: 'number',
    transactions: struct.list([
      struct.tuple([
        struct({
          signatures: struct.list(['string']),
          message: struct({
            accountKeys: struct.list(['string']),
            header: struct({
              numRequiredSignatures: 'number',
              numReadonlySignedAccounts: 'number',
              numReadonlyUnsignedAccounts: 'number',
            }),
            instructions: struct.list([
              struct.union([
                struct.list(['number']),
                struct({
                  accounts: struct.list(['number']),
                  data: 'string',
                  programIdIndex: 'number',
                }),
              ]),
            ]),
            recentBlockhash: 'string',
          }),
        }),
        struct.union([
          'null',
          struct({
            status: SignatureStatusResult,
            fee: 'number',
            preBalances: struct.list(['number']),
            postBalances: struct.list(['number']),
          }),
        ]),
      ]),
    ]),
  }),
);

/**
 * Expected JSON RPC response for the "getRecentBlockhash" message
 */
const GetRecentBlockhashAndContextRpcResult = jsonRpcResultAndContext([
  'string',
  struct({
    burnPercent: 'number',
    lamportsPerSignature: 'number',
    maxLamportsPerSignature: 'number',
    minLamportsPerSignature: 'number',
    targetLamportsPerSignature: 'number',
    targetSignaturesPerSlot: 'number',
  }),
]);

/**
 * Expected JSON RPC response for the "requestAirdrop" message
 */
const RequestAirdropRpcResult = jsonRpcResult('string');

/**
 * Expected JSON RPC response for the "sendTransaction" message
 */
const SendTransactionRpcResult = jsonRpcResult('string');

/**
 * Information describing an account
 *
 * @typedef {Object} AccountInfo
 * @property {number} lamports Number of lamports assigned to the account
 * @property {PublicKey} owner Identifier of the program that owns the account
 * @property {?Buffer} data Optional data assigned to the account
 * @property {boolean} executable `true` if this account's data contains a loaded program
 */
type AccountInfo = {
  executable: boolean,
  owner: PublicKey,
  lamports: number,
  data: Buffer,
};

/**
 * Account information identified by pubkey
 *
 * @typedef {Object} KeyedAccountInfo
 * @property {PublicKey} accountId
 * @property {AccountInfo} accountInfo
 */
type KeyedAccountInfo = {
  accountId: PublicKey,
  accountInfo: AccountInfo,
};

/**
 * Callback function for account change notifications
 */
export type AccountChangeCallback = (accountInfo: AccountInfo) => void;

/**
 * @private
 */
type AccountSubscriptionInfo = {
  publicKey: string, // PublicKey of the account as a base 58 string
  callback: AccountChangeCallback,
  subscriptionId: null | number, // null when there's no current server subscription id
};

/**
 * Callback function for program account change notifications
 */
export type ProgramAccountChangeCallback = (
  keyedAccountInfo: KeyedAccountInfo,
) => void;

/**
 * @private
 */
type ProgramAccountSubscriptionInfo = {
  programId: string, // PublicKey of the program as a base 58 string
  callback: ProgramAccountChangeCallback,
  subscriptionId: null | number, // null when there's no current server subscription id
};

/**
 * @private
 */
type SlotSubscriptionInfo = {
  callback: SlotChangeCallback,
  subscriptionId: null | number, // null when there's no current server subscription id
};

/**
 * Callback function for slot change notifications
 */
export type SlotChangeCallback = (slotInfo: SlotInfo) => void;

/**
 * Signature status: Success
 *
 * @typedef {Object} SignatureSuccess
 */
export type SignatureSuccess = {|
  Ok: null,
|};

/**
 * Signature status: TransactionError
 *
 * @typedef {Object} TransactionError
 */
export type TransactionError = {|
  Err: Object,
|};

/**
 * @ignore
 */
type BlockhashAndFeeCalculator = [Blockhash, FeeCalculator]; // This type exists to workaround an esdoc parse error

/**
 * @ignore
 */
type PublicKeyAndAccount = [PublicKey, AccountInfo]; // This type exists to workaround an esdoc parse error

/**
 * A connection to a fullnode JSON RPC endpoint
 */
export class Connection {
  _rpcRequest: RpcRequest;
  _rpcWebSocket: RpcWebSocketClient;
  _rpcWebSocketConnected: boolean = false;

  _commitment: ?Commitment;
  _blockhashInfo: {
    recentBlockhash: Blockhash | null,
    seconds: number,
    transactionSignatures: Array<string>,
  };
  _disableBlockhashCaching: boolean = false;
  _accountChangeSubscriptions: {[number]: AccountSubscriptionInfo} = {};
  _accountChangeSubscriptionCounter: number = 0;
  _programAccountChangeSubscriptions: {
    [number]: ProgramAccountSubscriptionInfo,
  } = {};
  _programAccountChangeSubscriptionCounter: number = 0;
  _slotSubscriptions: {
    [number]: SlotSubscriptionInfo,
  } = {};
  _slotSubscriptionCounter: number = 0;

  /**
   * Establish a JSON RPC connection
   *
   * @param endpoint URL to the fullnode JSON RPC endpoint
   * @param commitment optional default commitment level
   */
  constructor(endpoint: string, commitment: ?Commitment) {
    let url = urlParse(endpoint);

    this._rpcRequest = createRpcRequest(url.href);
    this._commitment = commitment;
    this._blockhashInfo = {
      recentBlockhash: null,
      seconds: -1,
      transactionSignatures: [],
    };

    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    url.host = '';
    url.port = String(Number(url.port) + 1);
    if (url.port === '1') {
      url.port = url.protocol === 'wss:' ? '8901' : '8900';
    }
    this._rpcWebSocket = new RpcWebSocketClient(urlFormat(url), {
      autoconnect: false,
      max_reconnects: Infinity,
    });
    this._rpcWebSocket.on('open', this._wsOnOpen.bind(this));
    this._rpcWebSocket.on('error', this._wsOnError.bind(this));
    this._rpcWebSocket.on('close', this._wsOnClose.bind(this));
    this._rpcWebSocket.on(
      'accountNotification',
      this._wsOnAccountNotification.bind(this),
    );
    this._rpcWebSocket.on(
      'programNotification',
      this._wsOnProgramAccountNotification.bind(this),
    );
    this._rpcWebSocket.on(
      'slotNotification',
      this._wsOnSlotNotification.bind(this),
    );
  }

  /**
   * Fetch the balance for the specified public key, return with context
   */
  async getBalanceAndContext(
    publicKey: PublicKey,
    commitment: ?Commitment,
  ): Promise<RpcResponseAndContext<number>> {
    const args = this._argsWithCommitment([publicKey.toBase58()], commitment);
    const unsafeRes = await this._rpcRequest('getBalance', args);
    const res = GetBalanceAndContextRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Fetch the balance for the specified public key
   */
  async getBalance(
    publicKey: PublicKey,
    commitment: ?Commitment,
  ): Promise<number> {
    return await this.getBalanceAndContext(publicKey, commitment)
      .then(x => x.value)
      .catch(e => {
        throw e;
      });
  }

  /**
   * Fetch all the account info for the specified public key, return with context
   */
  async getAccountInfoAndContext(
    publicKey: PublicKey,
    commitment: ?Commitment,
  ): Promise<RpcResponseAndContext<AccountInfo>> {
    const args = this._argsWithCommitment([publicKey.toBase58()], commitment);
    const unsafeRes = await this._rpcRequest('getAccountInfo', args);
    const res = GetAccountInfoAndContextRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');

    if (!res.result.value) {
      throw new Error('Invalid request');
    }

    const {executable, owner, lamports, data} = res.result.value;
    const value = {
      executable,
      owner: new PublicKey(owner),
      lamports,
      data: Buffer.from(data),
    };

    return {
      context: {
        slot: res.result.context.slot,
      },
      value,
    };
  }

  /**
   * Fetch all the account info for the specified public key
   */
  async getAccountInfo(
    publicKey: PublicKey,
    commitment: ?Commitment,
  ): Promise<AccountInfo> {
    return await this.getAccountInfoAndContext(publicKey, commitment)
      .then(x => x.value)
      .catch(e => {
        throw e;
      });
  }

  /**
   * Fetch all the accounts owned by the specified program id
   */
  async getProgramAccounts(
    programId: PublicKey,
    commitment: ?Commitment,
  ): Promise<Array<PublicKeyAndAccount>> {
    const args = this._argsWithCommitment([programId.toBase58()], commitment);
    const unsafeRes = await this._rpcRequest('getProgramAccounts', args);
    const res = GetProgramAccountsRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }

    const {result} = res;
    assert(typeof result !== 'undefined');

    return result.map(result => {
      return [
        result[0],
        {
          executable: result[1].executable,
          owner: new PublicKey(result[1].owner),
          lamports: result[1].lamports,
          data: Buffer.from(result[1].data),
        },
      ];
    });
  }

  /**
   * Confirm the transaction identified by the specified signature, return with context
   */
  async confirmTransactionAndContext(
    signature: TransactionSignature,
    commitment: ?Commitment,
  ): Promise<RpcResponseAndContext<boolean>> {
    const args = this._argsWithCommitment([signature], commitment);
    const unsafeRes = await this._rpcRequest('confirmTransaction', args);
    const res = ConfirmTransactionAndContextRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Confirm the transaction identified by the specified signature
   */
  async confirmTransaction(
    signature: TransactionSignature,
    commitment: ?Commitment,
  ): Promise<boolean> {
    return await this.confirmTransactionAndContext(signature, commitment)
      .then(x => x.value)
      .catch(e => {
        throw e;
      });
  }

  /**
   * Return the list of nodes that are currently participating in the cluster
   */
  async getClusterNodes(): Promise<Array<ContactInfo>> {
    const unsafeRes = await this._rpcRequest('getClusterNodes', []);

    const res = GetClusterNodes(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Return the list of nodes that are currently participating in the cluster
   */
  async getVoteAccounts(commitment: ?Commitment): Promise<VoteAccountStatus> {
    const args = this._argsWithCommitment([], commitment);
    const unsafeRes = await this._rpcRequest('getVoteAccounts', args);
    const res = GetVoteAccounts(unsafeRes);
    //const res = unsafeRes;
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Fetch the current slot that the node is processing
   */
  async getSlot(commitment: ?Commitment): Promise<number> {
    const args = this._argsWithCommitment([], commitment);
    const unsafeRes = await this._rpcRequest('getSlot', args);
    const res = GetSlot(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Fetch the current slot leader of the cluster
   */
  async getSlotLeader(commitment: ?Commitment): Promise<string> {
    const args = this._argsWithCommitment([], commitment);
    const unsafeRes = await this._rpcRequest('getSlotLeader', args);
    const res = GetSlotLeader(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Fetch the current transaction count of the cluster
   */
  async getSignatureStatus(
    signature: TransactionSignature,
    commitment: ?Commitment,
  ): Promise<SignatureSuccess | TransactionError | null> {
    const args = this._argsWithCommitment([signature], commitment);
    const unsafeRes = await this._rpcRequest('getSignatureStatus', args);
    const res = GetSignatureStatusRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Fetch the current transaction count of the cluster
   */
  async getTransactionCount(commitment: ?Commitment): Promise<number> {
    const args = this._argsWithCommitment([], commitment);
    const unsafeRes = await this._rpcRequest('getTransactionCount', args);
    const res = GetTransactionCountRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return Number(res.result);
  }

  /**
   * Fetch the current total currency supply of the cluster in lamports
   */
  async getTotalSupply(commitment: ?Commitment): Promise<number> {
    const args = this._argsWithCommitment([], commitment);
    const unsafeRes = await this._rpcRequest('getTotalSupply', args);
    const res = GetTotalSupplyRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return Number(res.result);
  }

  /**
   * Fetch the cluster Inflation parameters
   */
  async getInflation(commitment: ?Commitment): Promise<GetInflationRpcResult> {
    const args = this._argsWithCommitment([], commitment);
    const unsafeRes = await this._rpcRequest('getInflation', args);
    const res = GetInflationRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return GetInflationResult(res.result);
  }

  /**
   * Fetch the Epoch Info parameters
   */
  async getEpochInfo(commitment: ?Commitment): Promise<GetEpochInfoRpcResult> {
    const args = this._argsWithCommitment([], commitment);
    const unsafeRes = await this._rpcRequest('getEpochInfo', args);
    const res = GetEpochInfoRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return GetEpochInfoResult(res.result);
  }

  /**
   * Fetch the Epoch Schedule parameters
   */
  async getEpochSchedule(): Promise<GetEpochScheduleRpcResult> {
    const unsafeRes = await this._rpcRequest('getEpochSchedule', []);
    const res = GetEpochScheduleRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return GetEpochScheduleResult(res.result);
  }

  /**
   * Fetch the minimum balance needed to exempt an account of `dataLength`
   * size from rent
   */
  async getMinimumBalanceForRentExemption(
    dataLength: number,
    commitment: ?Commitment,
  ): Promise<number> {
    const args = this._argsWithCommitment([dataLength], commitment);
    const unsafeRes = await this._rpcRequest(
      'getMinimumBalanceForRentExemption',
      args,
    );
    const res = GetMinimumBalanceForRentExemptionRpcResult(unsafeRes);
    if (res.error) {
      console.warn('Unable to fetch minimum balance for rent exemption');
      return 0;
    }
    assert(typeof res.result !== 'undefined');
    return Number(res.result);
  }

  /**
   * Fetch a recent blockhash from the cluster, return with context
   */
  async getRecentBlockhashAndContext(
    commitment: ?Commitment,
  ): Promise<RpcResponseAndContext<BlockhashAndFeeCalculator>> {
    const args = this._argsWithCommitment([], commitment);
    const unsafeRes = await this._rpcRequest('getRecentBlockhash', args);

    const res = GetRecentBlockhashAndContextRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Fetch a recent blockhash from the cluster
   */
  async getRecentBlockhash(
    commitment: ?Commitment,
  ): Promise<BlockhashAndFeeCalculator> {
    return await this.getRecentBlockhashAndContext(commitment)
      .then(x => x.value)
      .catch(e => {
        throw e;
      });
  }

  /**
   * Fetch the node version
   */
  async getVersion(): Promise<Version> {
    const unsafeRes = await this._rpcRequest('getVersion', []);
    const res = GetVersionRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Fetch a list of Transactions and transaction statuses from the cluster
   * for a confirmed block
   */
  async getConfirmedBlock(slot: number): Promise<ConfirmedBlock> {
    const unsafeRes = await this._rpcRequest('getConfirmedBlock', [slot]);
    const result = GetConfirmedBlockRpcResult(unsafeRes);
    if (result.error) {
      throw new Error(result.error.message);
    }
    assert(typeof result.result !== 'undefined');
    return {
      blockhash: new PublicKey(result.result.blockhash).toString(),
      previousBlockhash: new PublicKey(
        result.result.previousBlockhash,
      ).toString(),
      parentSlot: result.result.parentSlot,
      transactions: result.result.transactions.map(result => {
        return [Transaction.fromRpcResult(result[0]), result[1]];
      }),
    };
  }

  /**
   * Fetch the contents of a Nonce account from the cluster, return with context
   */
  async getNonceAndContext(
    nonceAccount: PublicKey,
    commitment: ?Commitment,
  ): Promise<RpcResponseAndContext<NonceAccount>> {
    const args = this._argsWithCommitment(
      [nonceAccount.toBase58()],
      commitment,
    );
    const unsafeRes = await this._rpcRequest('getAccountInfo', args);
    const res = GetAccountInfoAndContextRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    if (!res.result.value) {
      throw new Error('Invalid request');
    }

    const value = NonceAccount.fromAccountData(
      Buffer.from(res.result.value.data),
    );

    return {
      context: {
        slot: res.result.context.slot,
      },
      value,
    };
  }

  /**
   * Fetch the contents of a Nonce account from the cluster
   */
  async getNonce(
    nonceAccount: PublicKey,
    commitment: ?Commitment,
  ): Promise<NonceAccount> {
    return await this.getNonceAndContext(nonceAccount, commitment)
      .then(x => x.value)
      .catch(e => {
        throw e;
      });
  }

  /**
   * Request an allocation of lamports to the specified account
   */
  async requestAirdrop(
    to: PublicKey,
    amount: number,
    commitment: ?Commitment,
  ): Promise<TransactionSignature> {
    const args = this._argsWithCommitment([to.toBase58(), amount], commitment);
    const unsafeRes = await this._rpcRequest('requestAirdrop', args);
    const res = RequestAirdropRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Sign and send a transaction
   */
  async sendTransaction(
    transaction: Transaction,
    ...signers: Array<Account>
  ): Promise<TransactionSignature> {
    if (transaction.nonceInfo) {
      transaction.sign(...signers);
    } else {
      for (;;) {
        // Attempt to use a recent blockhash for up to 30 seconds
        const seconds = new Date().getSeconds();
        if (
          this._blockhashInfo.recentBlockhash != null &&
          this._blockhashInfo.seconds < seconds + 30
        ) {
          transaction.recentBlockhash = this._blockhashInfo.recentBlockhash;
          transaction.sign(...signers);
          if (!transaction.signature) {
            throw new Error('!signature'); // should never happen
          }

          // If the signature of this transaction has not been seen before with the
          // current recentBlockhash, all done.
          const signature = transaction.signature.toString();
          if (!this._blockhashInfo.transactionSignatures.includes(signature)) {
            this._blockhashInfo.transactionSignatures.push(signature);
            if (this._disableBlockhashCaching) {
              this._blockhashInfo.seconds = -1;
            }
            break;
          }
        }

        // Fetch a new blockhash
        let attempts = 0;
        const startTime = Date.now();
        for (;;) {
          const [
            recentBlockhash,
            //feeCalculator,
          ] = await this.getRecentBlockhash();

          if (this._blockhashInfo.recentBlockhash != recentBlockhash) {
            this._blockhashInfo = {
              recentBlockhash,
              seconds: new Date().getSeconds(),
              transactionSignatures: [],
            };
            break;
          }
          if (attempts === 50) {
            throw new Error(
              `Unable to obtain a new blockhash after ${Date.now() -
                startTime}ms`,
            );
          }

          // Sleep for approximately half a slot
          await sleep((500 * DEFAULT_TICKS_PER_SLOT) / NUM_TICKS_PER_SECOND);

          ++attempts;
        }
      }
    }

    const wireTransaction = transaction.serialize();
    return await this.sendRawTransaction(wireTransaction);
  }

  /**
   * @private
   */
  async validatorExit(): Promise<boolean> {
    const unsafeRes = await this._rpcRequest('validatorExit', []);
    const res = jsonRpcResult('boolean')(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    return res.result;
  }

  /**
   * Send a transaction that has already been signed and serialized into the
   * wire format
   */
  async sendRawTransaction(
    rawTransaction: Buffer,
  ): Promise<TransactionSignature> {
    const unsafeRes = await this._rpcRequest('sendTransaction', [
      [...rawTransaction],
    ]);
    const res = SendTransactionRpcResult(unsafeRes);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    assert(res.result);
    return res.result;
  }

  /**
   * @private
   */
  _wsOnOpen() {
    this._rpcWebSocketConnected = true;
    this._updateSubscriptions();
  }

  /**
   * @private
   */
  _wsOnError(err: Error) {
    console.log('ws error:', err.message);
  }

  /**
   * @private
   */
  _wsOnClose(code: number, message: string) {
    // 1000 means _rpcWebSocket.close() was called explicitly
    if (code !== 1000) {
      console.log('ws close:', code, message);
    } else {
      // Only after an explicit close do we need to explicitly connect again
      this._rpcWebSocketConnected = false;
    }
  }

  /**
   * @private
   */
  async _updateSubscriptions() {
    const accountKeys = Object.keys(this._accountChangeSubscriptions).map(
      Number,
    );
    const programKeys = Object.keys(
      this._programAccountChangeSubscriptions,
    ).map(Number);
    const slotKeys = Object.keys(this._slotSubscriptions).map(Number);
    if (
      accountKeys.length === 0 &&
      programKeys.length === 0 &&
      slotKeys.length === 0
    ) {
      this._rpcWebSocket.close();
      return;
    }

    if (!this._rpcWebSocketConnected) {
      for (let id of accountKeys) {
        this._accountChangeSubscriptions[id].subscriptionId = null;
      }
      for (let id of programKeys) {
        this._programAccountChangeSubscriptions[id].subscriptionId = null;
      }
      for (let id of slotKeys) {
        this._slotSubscriptions[id].subscriptionId = null;
      }
      this._rpcWebSocket.connect();
      return;
    }

    for (let id of accountKeys) {
      const {subscriptionId, publicKey} = this._accountChangeSubscriptions[id];
      if (subscriptionId === null) {
        try {
          this._accountChangeSubscriptions[
            id
          ].subscriptionId = await this._rpcWebSocket.call('accountSubscribe', [
            publicKey,
          ]);
        } catch (err) {
          console.log(
            `accountSubscribe error for ${publicKey}: ${err.message}`,
          );
        }
      }
    }
    for (let id of programKeys) {
      const {
        subscriptionId,
        programId,
      } = this._programAccountChangeSubscriptions[id];
      if (subscriptionId === null) {
        try {
          this._programAccountChangeSubscriptions[
            id
          ].subscriptionId = await this._rpcWebSocket.call('programSubscribe', [
            programId,
          ]);
        } catch (err) {
          console.log(
            `programSubscribe error for ${programId}: ${err.message}`,
          );
        }
      }
    }
    for (let id of slotKeys) {
      const {subscriptionId} = this._slotSubscriptions[id];
      if (subscriptionId === null) {
        try {
          this._slotSubscriptions[
            id
          ].subscriptionId = await this._rpcWebSocket.call('slotSubscribe', []);
        } catch (err) {
          console.log(`slotSubscribe error: ${err.message}`);
        }
      }
    }
  }

  /**
   * @private
   */
  _wsOnAccountNotification(notification: Object) {
    const res = AccountNotificationResult(notification);
    if (res.error) {
      throw new Error(res.error.message);
    }

    const keys = Object.keys(this._accountChangeSubscriptions).map(Number);
    for (let id of keys) {
      const sub = this._accountChangeSubscriptions[id];
      if (sub.subscriptionId === res.subscription) {
        const {result} = res;
        assert(typeof result !== 'undefined');

        sub.callback({
          executable: result.executable,
          owner: new PublicKey(result.owner),
          lamports: result.lamports,
          data: Buffer.from(result.data),
        });
        return true;
      }
    }
  }

  /**
   * Register a callback to be invoked whenever the specified account changes
   *
   * @param publickey Public key of the account to monitor
   * @param callback Function to invoke whenever the account is changed
   * @return subscription id
   */
  onAccountChange(
    publicKey: PublicKey,
    callback: AccountChangeCallback,
  ): number {
    const id = ++this._accountChangeSubscriptionCounter;
    this._accountChangeSubscriptions[id] = {
      publicKey: publicKey.toBase58(),
      callback,
      subscriptionId: null,
    };
    this._updateSubscriptions();
    return id;
  }

  /**
   * Deregister an account notification callback
   *
   * @param id subscription id to deregister
   */
  async removeAccountChangeListener(id: number): Promise<void> {
    if (this._accountChangeSubscriptions[id]) {
      const {subscriptionId} = this._accountChangeSubscriptions[id];
      delete this._accountChangeSubscriptions[id];
      if (subscriptionId !== null) {
        try {
          await this._rpcWebSocket.call('accountUnsubscribe', [subscriptionId]);
        } catch (err) {
          console.log('accountUnsubscribe error:', err.message);
        }
      }
      this._updateSubscriptions();
    } else {
      throw new Error(`Unknown account change id: ${id}`);
    }
  }

  /**
   * @private
   */
  _wsOnProgramAccountNotification(notification: Object) {
    const res = ProgramAccountNotificationResult(notification);
    if (res.error) {
      throw new Error(res.error.message);
    }

    const keys = Object.keys(this._programAccountChangeSubscriptions).map(
      Number,
    );
    for (let id of keys) {
      const sub = this._programAccountChangeSubscriptions[id];
      if (sub.subscriptionId === res.subscription) {
        const {result} = res;
        assert(typeof result !== 'undefined');

        sub.callback({
          accountId: result[0],
          accountInfo: {
            executable: result[1].executable,
            owner: new PublicKey(result[1].owner),
            lamports: result[1].lamports,
            data: Buffer.from(result[1].data),
          },
        });
        return true;
      }
    }
  }

  /**
   * Register a callback to be invoked whenever accounts owned by the
   * specified program change
   *
   * @param programId Public key of the program to monitor
   * @param callback Function to invoke whenever the account is changed
   * @return subscription id
   */
  onProgramAccountChange(
    programId: PublicKey,
    callback: ProgramAccountChangeCallback,
  ): number {
    const id = ++this._programAccountChangeSubscriptionCounter;
    this._programAccountChangeSubscriptions[id] = {
      programId: programId.toBase58(),
      callback,
      subscriptionId: null,
    };
    this._updateSubscriptions();
    return id;
  }

  /**
   * Deregister an account notification callback
   *
   * @param id subscription id to deregister
   */
  async removeProgramAccountChangeListener(id: number): Promise<void> {
    if (this._programAccountChangeSubscriptions[id]) {
      const {subscriptionId} = this._programAccountChangeSubscriptions[id];
      delete this._programAccountChangeSubscriptions[id];
      if (subscriptionId !== null) {
        try {
          await this._rpcWebSocket.call('programUnsubscribe', [subscriptionId]);
        } catch (err) {
          console.log('programUnsubscribe error:', err.message);
        }
      }
      this._updateSubscriptions();
    } else {
      throw new Error(`Unknown account change id: ${id}`);
    }
  }

  /**
   * @private
   */
  _wsOnSlotNotification(notification: Object) {
    const res = SlotNotificationResult(notification);
    if (res.error) {
      throw new Error(res.error.message);
    }
    assert(typeof res.result !== 'undefined');
    const {parent, slot, root} = res.result;

    const keys = Object.keys(this._slotSubscriptions).map(Number);
    for (let id of keys) {
      const sub = this._slotSubscriptions[id];
      if (sub.subscriptionId === res.subscription) {
        sub.callback({
          parent,
          slot,
          root,
        });
        return true;
      }
    }
  }

  /**
   * Register a callback to be invoked upon slot changes
   *
   * @param callback Function to invoke whenever the slot changes
   * @return subscription id
   */
  onSlotChange(callback: SlotChangeCallback): number {
    const id = ++this._slotSubscriptionCounter;
    this._slotSubscriptions[id] = {
      callback,
      subscriptionId: id,
    };
    this._updateSubscriptions();
    return id;
  }

  /**
   * Deregister a slot notification callback
   *
   * @param id subscription id to deregister
   */
  async removeSlotChangeListener(id: number): Promise<void> {
    if (this._slotSubscriptions[id]) {
      const {subscriptionId} = this._slotSubscriptions[id];
      delete this._slotSubscriptions[id];
      if (subscriptionId !== null) {
        try {
          await this._rpcWebSocket.call('slotUnsubscribe', [subscriptionId]);
        } catch (err) {
          console.log('slotUnsubscribe error:', err.message);
        }
      }
      this._updateSubscriptions();
    } else {
      throw new Error(`Unknown slot change id: ${id}`);
    }
  }

  _argsWithCommitment(args: Array<any>, override: ?Commitment): Array<any> {
    const commitment = override || this._commitment;
    if (commitment) {
      args.push({commitment});
    }
    return args;
  }
}
