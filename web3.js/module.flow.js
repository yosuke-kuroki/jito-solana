/**
 * Flow Library definition for @solana/web3.js
 *
 * This file is manually generated from the contents of src/
 *
 * Usage: add the following line under the [libs] section of your project's
 * .flowconfig:
 * [libs]
 * node_modules/@solana/web3.js/module.flow.js
 *
 */

import BN from 'bn.js';

declare module '@solana/web3.js' {
  // === src/publickey.js ===
  declare export class PublicKey {
    constructor(number: string | Buffer | Array<number>): PublicKey;
    static isPublicKey(o: Object): boolean;
    equals(publickey: PublicKey): boolean;
    toBase58(): string;
    toBuffer(): Buffer;
  }

  // === src/account.js ===
  declare export class Account {
    constructor(secretKey: ?Buffer): Account;
    publicKey: PublicKey;
    secretKey: Buffer;
  }

  // === src/budget-program.js ===
  /* TODO */

  // === src/connection.js ===
  declare export type AccountInfo = {
    executable: boolean;
    loaderProgramId: PublicKey,
    programId: PublicKey,
    tokens: number,
    userdata: Buffer,
  }

  declare type AccountChangeCallback = (accountInfo: AccountInfo) => void;

  declare export type SignatureStatus = 'Confirmed'
    | 'AccountInUse'
    | 'SignatureNotFound'
    | 'ProgramRuntimeError'
    | 'GenericFailure';

  declare export class Connection {
    constructor(endpoint: string): Connection;
    getBalance(publicKey: PublicKey): Promise<number>;
    getAccountInfo(publicKey: PublicKey): Promise<AccountInfo>;
    confirmTransaction(signature: TransactionSignature): Promise<boolean>;
    getSignatureStatus(signature: TransactionSignature): Promise<SignatureStatus>;
    getTransactionCount(): Promise<number>;
    getLastId(): Promise<TransactionId>;
    getFinality(): Promise<number>;
    requestAirdrop(to: PublicKey, amount: number): Promise<TransactionSignature>;
    sendTransaction(from: Account, transaction: Transaction): Promise<TransactionSignature>;
    onAccountChange(publickey: PublicKey, callback: AccountChangeCallback): Promise<number>;
    removeAccountListener(id: number): Promise<void>;
  }

  // === src/system-program.js ===
  declare export class SystemProgram {
    static programId: PublicKey;

    static createAccount(
      from: PublicKey,
      newAccount: PublicKey,
      tokens: number,
      space: number,
      programId: PublicKey
    ): Transaction;
    static move(from: PublicKey, to: PublicKey, amount: number): Transaction;
    static assign(from: PublicKey, programId: PublicKey): Transaction;
    static spawn(programId: PublicKey): Transaction;
  }

  // === src/transaction.js ===
  declare export type TransactionSignature = string;
  declare export type TransactionId = string;

  declare type TransactionInstructionCtorFields = {|
    keys?: Array<PublicKey>;
    programId?: PublicKey;
    userdata?: Buffer;
  |};

  declare export class TransactionInstruction {
    fee: number;

    constructor(opts?: TransactionInstructionCtorFields): TransactionInstruction;
  }

  declare type TransactionCtorFields = {|
    fee?: number;
  |};

  declare export class Transaction {
    signature: ?Buffer;
    fee: number;

    constructor(opts?: TransactionCtorFields): Transaction;
    add(item: TransactionInstruction | TransactionInstructionCtorFields): Transaction;
    sign(from: Account): void;
    serialize(): Buffer;
  }

  // === src/token-program.js ===
  declare export class TokenAmount extends BN {
    toBuffer(): Buffer;
    fromBuffer(buffer: Buffer): TokenAmount;
  }

  declare export type TokenInfo = {|
    supply: TokenAmount,
    decimals: number,
    name: string,
    symbol: string,
  |};
  declare export type TokenAccountInfo = {|
    token: PublicKey;
    owner: PublicKey;
    amount: TokenAmount;
    source: null | PublicKey;
    originalAmount: TokenAmount;
  |}
  declare type TokenAndPublicKey = [Token, PublicKey];

  declare export class Token {
    programId: PublicKey;
    token: PublicKey;

    static createNewToken(
      connection: Connection,
      owner: Account,
      supply: TokenAmount,
      name: string,
      symbol: string,
      decimals: number,
      programId?: PublicKey,
    ): Promise<TokenAndPublicKey>;

    constructor(connection: Connection, token: PublicKey) : Token;
    newAccount(owner: Account, source?: PublicKey): Promise<PublicKey>;
    tokenInfo(): Promise<TokenInfo>;
    accountInfo(account: PublicKey): Promise<TokenAccountInfo>;
    transfer(
      owner: Account,
      source: PublicKey,
      destination: PublicKey,
      amount: number | TokenAmount,
    ): Promise<void>;
    approve(
      owner: Account,
      account: PublicKey,
      delegate: PublicKey,
      amount: number | TokenAmount
    ): Promise<void>;
    revoke(
      owner: Account,
      account: PublicKey,
      delegate: PublicKey
    ): Promise<void>;
    setOwner(
      owner: Account,
      account: PublicKey,
      newOwner: PublicKey
    ): Promise<void>;

    transferInstruction(
      owner: PublicKey,
      source: PublicKey,
      destination: PublicKey,
      amount: number | TokenAmount,
    ): Promise<TransactionInstruction>;
    approveInstruction(
      owner: PublicKey,
      account: PublicKey,
      delegate: PublicKey,
      amount: number | TokenAmount
    ): TransactionInstruction;
    revokeInstruction(
      owner: PublicKey,
      account: PublicKey,
      delegate: PublicKey,
    ): TransactionInstruction;
    setOwnerInstruction(
      owner: PublicKey,
      account: PublicKey,
      newOwner: PublicKey,
    ): TransactionInstruction;
  }

  // === src/loader.js ===
  declare export class Loader {
    constructor(connection: Connection, programId: PublicKey) : Loader;
    load(program: Account, offset: number, bytes: Array<number>): Promise<void>;
    finalize(program: Account): Promise<void>;
  }

  // === src/native-loader.js ===
  declare export class NativeLoader {
    static programId: PublicKey;
    static load(
      connection: Connection,
      owner: Account,
      programName: string,
    ): Promise<PublicKey>;
  }

  // === src/util/send-and-confirm-transaction.js ===
  declare export function sendAndConfirmTransaction(
    connection: Connection,
    from: Account,
    transaction: Transaction,
    runtimeErrorOk?: boolean,
  ): Promise<void>;
}
