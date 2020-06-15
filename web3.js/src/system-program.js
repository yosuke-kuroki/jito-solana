// @flow

import * as BufferLayout from 'buffer-layout';

import {encodeData} from './instruction';
import * as Layout from './layout';
import {PublicKey} from './publickey';
import {SYSVAR_RECENT_BLOCKHASHES_PUBKEY, SYSVAR_RENT_PUBKEY} from './sysvar';
import {Transaction, TransactionInstruction} from './transaction';
import type {TransactionInstructionCtorFields} from './transaction';

/**
 * System Instruction class
 */
export class SystemInstruction extends TransactionInstruction {
  /**
   * Type of SystemInstruction
   */
  _type: SystemInstructionType;

  constructor(
    opts?: TransactionInstructionCtorFields,
    type: SystemInstructionType,
  ) {
    if (
      opts &&
      opts.programId &&
      !opts.programId.equals(SystemProgram.programId)
    ) {
      throw new Error('programId incorrect; not a SystemInstruction');
    }
    super(opts);
    this._type = type;
  }

  static from(instruction: TransactionInstruction): SystemInstruction {
    if (!instruction.programId.equals(SystemProgram.programId)) {
      throw new Error('programId incorrect; not SystemProgram');
    }

    const instructionTypeLayout = BufferLayout.u32('instruction');
    const typeIndex = instructionTypeLayout.decode(instruction.data);
    let type;
    for (const t of Object.keys(SYSTEM_INSTRUCTION_LAYOUTS)) {
      if (SYSTEM_INSTRUCTION_LAYOUTS[t].index == typeIndex) {
        type = t;
      }
    }
    if (!type) {
      throw new Error('Instruction type incorrect; not a SystemInstruction');
    }
    return new SystemInstruction(
      {
        keys: instruction.keys,
        programId: instruction.programId,
        data: instruction.data,
      },
      type,
    );
  }

  /**
   * Type of SystemInstruction
   */
  get type(): SystemInstructionType {
    return this._type;
  }

  /**
   * The `from` public key of the instruction;
   * returns null if SystemInstructionType does not support this field
   */
  get fromPublicKey(): PublicKey | null {
    switch (this.type) {
      case 'Create':
      case 'CreateWithSeed':
      case 'WithdrawNonceAccount':
      case 'Transfer':
        return this.keys[0].pubkey;
      default:
        return null;
    }
  }

  /**
   * The `to` public key of the instruction;
   * returns null if SystemInstructionType does not support this field
   */
  get toPublicKey(): PublicKey | null {
    switch (this.type) {
      case 'Create':
      case 'CreateWithSeed':
      case 'WithdrawNonceAccount':
      case 'Transfer':
        return this.keys[1].pubkey;
      default:
        return null;
    }
  }

  /**
   * The `amount` or `lamports` of the instruction;
   * returns null if SystemInstructionType does not support this field
   */
  get amount(): number | null {
    const data = SYSTEM_INSTRUCTION_LAYOUTS[this.type].layout.decode(this.data);
    switch (this.type) {
      case 'Create':
      case 'CreateWithSeed':
      case 'WithdrawNonceAccount':
      case 'Transfer':
        return data.lamports;
      default:
        return null;
    }
  }
}

/**
 * An enumeration of valid SystemInstructionType's
 * @typedef {'Create' | 'Assign' | 'Transfer' | 'CreateWithSeed'
 | 'AdvanceNonceAccount' | 'WithdrawNonceAccount' | 'InitializeNonceAccount'
 | 'AuthorizeNonceAccount'} SystemInstructionType
 */
export type SystemInstructionType = $Keys<typeof SYSTEM_INSTRUCTION_LAYOUTS>;

/**
 * An enumeration of valid system InstructionType's
 */
const SYSTEM_INSTRUCTION_LAYOUTS = Object.freeze({
  Create: {
    index: 0,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      BufferLayout.ns64('lamports'),
      BufferLayout.ns64('space'),
      Layout.publicKey('programId'),
    ]),
  },
  Assign: {
    index: 1,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      Layout.publicKey('programId'),
    ]),
  },
  Transfer: {
    index: 2,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      BufferLayout.ns64('lamports'),
    ]),
  },
  CreateWithSeed: {
    index: 3,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      Layout.publicKey('base'),
      Layout.rustString('seed'),
      BufferLayout.ns64('lamports'),
      BufferLayout.ns64('space'),
      Layout.publicKey('programId'),
    ]),
  },
  AdvanceNonceAccount: {
    index: 4,
    layout: BufferLayout.struct([BufferLayout.u32('instruction')]),
  },
  WithdrawNonceAccount: {
    index: 5,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      BufferLayout.ns64('lamports'),
    ]),
  },
  InitializeNonceAccount: {
    index: 6,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      Layout.publicKey('authorized'),
    ]),
  },
  AuthorizeNonceAccount: {
    index: 7,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      Layout.publicKey('authorized'),
    ]),
  },
});

/**
 * Factory class for transactions to interact with the System program
 */
export class SystemProgram {
  /**
   * Public key that identifies the System program
   */
  static get programId(): PublicKey {
    return new PublicKey(
      '0x000000000000000000000000000000000000000000000000000000000000000',
    );
  }

  /**
   * Max space of a Nonce account
   */
  static get nonceSpace(): number {
    return 68;
  }

  /**
   * Generate a Transaction that creates a new account
   */
  static createAccount(
    from: PublicKey,
    newAccount: PublicKey,
    lamports: number,
    space: number,
    programId: PublicKey,
  ): Transaction {
    const type = SYSTEM_INSTRUCTION_LAYOUTS.Create;
    const data = encodeData(type, {
      lamports,
      space,
      programId: programId.toBuffer(),
    });

    return new Transaction().add({
      keys: [
        {pubkey: from, isSigner: true, isWritable: true},
        {pubkey: newAccount, isSigner: true, isWritable: true},
      ],
      programId: this.programId,
      data,
    });
  }

  /**
   * Generate a Transaction that transfers lamports from one account to another
   */
  static transfer(
    from: PublicKey,
    to: PublicKey,
    lamports: number,
  ): Transaction {
    const type = SYSTEM_INSTRUCTION_LAYOUTS.Transfer;
    const data = encodeData(type, {lamports});

    return new Transaction().add({
      keys: [
        {pubkey: from, isSigner: true, isWritable: true},
        {pubkey: to, isSigner: false, isWritable: true},
      ],
      programId: this.programId,
      data,
    });
  }

  /**
   * Generate a Transaction that assigns an account to a program
   */
  static assign(from: PublicKey, programId: PublicKey): Transaction {
    const type = SYSTEM_INSTRUCTION_LAYOUTS.Assign;
    const data = encodeData(type, {programId: programId.toBuffer()});

    return new Transaction().add({
      keys: [{pubkey: from, isSigner: true, isWritable: true}],
      programId: this.programId,
      data,
    });
  }

  /**
   * Generate a Transaction that creates a new account at
   *   an address generated with `from`, a seed, and programId
   */
  static createAccountWithSeed(
    from: PublicKey,
    newAccount: PublicKey,
    base: PublicKey,
    seed: string,
    lamports: number,
    space: number,
    programId: PublicKey,
  ): Transaction {
    const type = SYSTEM_INSTRUCTION_LAYOUTS.CreateWithSeed;
    const data = encodeData(type, {
      base: base.toBuffer(),
      seed,
      lamports,
      space,
      programId: programId.toBuffer(),
    });

    return new Transaction().add({
      keys: [
        {pubkey: from, isSigner: true, isWritable: true},
        {pubkey: newAccount, isSigner: false, isWritable: true},
      ],
      programId: this.programId,
      data,
    });
  }

  /**
   * Generate a Transaction that creates a new Nonce account
   */
  static createNonceAccount(
    from: PublicKey,
    nonceAccount: PublicKey,
    authorizedPubkey: PublicKey,
    lamports: number,
  ): Transaction {
    let transaction = SystemProgram.createAccount(
      from,
      nonceAccount,
      lamports,
      this.nonceSpace,
      this.programId,
    );

    const type = SYSTEM_INSTRUCTION_LAYOUTS.InitializeNonceAccount;
    const data = encodeData(type, {
      authorized: authorizedPubkey.toBuffer(),
    });

    return transaction.add({
      keys: [
        {pubkey: nonceAccount, isSigner: false, isWritable: true},
        {
          pubkey: SYSVAR_RECENT_BLOCKHASHES_PUBKEY,
          isSigner: false,
          isWritable: false,
        },
        {pubkey: SYSVAR_RENT_PUBKEY, isSigner: false, isWritable: false},
      ],
      programId: this.programId,
      data,
    });
  }

  /**
   * Generate an instruction to advance the nonce in a Nonce account
   */
  static nonceAdvance(
    nonceAccount: PublicKey,
    authorizedPubkey: PublicKey,
  ): TransactionInstruction {
    const type = SYSTEM_INSTRUCTION_LAYOUTS.AdvanceNonceAccount;
    const data = encodeData(type);
    const instructionData = {
      keys: [
        {pubkey: nonceAccount, isSigner: false, isWritable: true},
        {
          pubkey: SYSVAR_RECENT_BLOCKHASHES_PUBKEY,
          isSigner: false,
          isWritable: false,
        },
        {pubkey: authorizedPubkey, isSigner: true, isWritable: false},
      ],
      programId: this.programId,
      data,
    };
    return new TransactionInstruction(instructionData);
  }

  /**
   * Generate a Transaction that withdraws lamports from a Nonce account
   */
  static nonceWithdraw(
    nonceAccount: PublicKey,
    authorizedPubkey: PublicKey,
    to: PublicKey,
    lamports: number,
  ): Transaction {
    const type = SYSTEM_INSTRUCTION_LAYOUTS.WithdrawNonceAccount;
    const data = encodeData(type, {lamports});

    return new Transaction().add({
      keys: [
        {pubkey: nonceAccount, isSigner: false, isWritable: true},
        {pubkey: to, isSigner: false, isWritable: true},
        {
          pubkey: SYSVAR_RECENT_BLOCKHASHES_PUBKEY,
          isSigner: false,
          isWritable: false,
        },
        {
          pubkey: SYSVAR_RENT_PUBKEY,
          isSigner: false,
          isWritable: false,
        },
        {pubkey: authorizedPubkey, isSigner: true, isWritable: false},
      ],
      programId: this.programId,
      data,
    });
  }

  /**
   * Generate a Transaction that authorizes a new PublicKey as the authority
   * on a Nonce account.
   */
  static nonceAuthorize(
    nonceAccount: PublicKey,
    authorizedPubkey: PublicKey,
    newAuthorized: PublicKey,
  ): Transaction {
    const type = SYSTEM_INSTRUCTION_LAYOUTS.AuthorizeNonceAccount;
    const data = encodeData(type, {
      newAuthorized: newAuthorized.toBuffer(),
    });

    return new Transaction().add({
      keys: [
        {pubkey: nonceAccount, isSigner: false, isWritable: true},
        {pubkey: authorizedPubkey, isSigner: true, isWritable: false},
      ],
      programId: this.programId,
      data,
    });
  }
}
