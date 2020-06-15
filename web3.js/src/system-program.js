// @flow

import * as BufferLayout from 'buffer-layout';

import {Transaction, TransactionInstruction} from './transaction';
import {PublicKey} from './publickey';
import * as Layout from './layout';
import type {TransactionInstructionCtorFields} from './transaction';

/**
 * System Instruction class
 */
export class SystemInstruction extends TransactionInstruction {
  /**
   * Type of SystemInstruction
   */
  type: SystemInstructionType;

  constructor(
    opts?: TransactionInstructionCtorFields,
    type?: SystemInstructionType,
  ) {
    if (
      opts &&
      opts.programId &&
      !opts.programId.equals(SystemProgram.programId)
    ) {
      throw new Error('programId incorrect; not a SystemInstruction');
    }
    super(opts);
    if (type) {
      this.type = type;
    }
  }

  static from(instruction: TransactionInstruction): SystemInstruction {
    if (!instruction.programId.equals(SystemProgram.programId)) {
      throw new Error('programId incorrect; not SystemProgram');
    }

    const instructionTypeLayout = BufferLayout.u32('instruction');
    const typeIndex = instructionTypeLayout.decode(instruction.data);
    let type;
    for (const t in SystemInstructionEnum) {
      if (SystemInstructionEnum[t].index == typeIndex) {
        type = SystemInstructionEnum[t];
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
   * The `from` public key of the instruction;
   * returns null if SystemInstructionType does not support this field
   */
  get From(): PublicKey | null {
    if (
      this.type == SystemInstructionEnum.CREATE ||
      this.type == SystemInstructionEnum.TRANSFER
    ) {
      return this.keys[0].pubkey;
    }
    return null;
  }

  /**
   * The `to` public key of the instruction;
   * returns null if SystemInstructionType does not support this field
   */
  get To(): PublicKey | null {
    if (
      this.type == SystemInstructionEnum.CREATE ||
      this.type == SystemInstructionEnum.TRANSFER
    ) {
      return this.keys[1].pubkey;
    }
    return null;
  }

  /**
   * The `amount` or `lamports` of the instruction;
   * returns null if SystemInstructionType does not support this field
   */
  get Amount(): number | null {
    const data = this.type.layout.decode(this.data);
    if (this.type == SystemInstructionEnum.TRANSFER) {
      return data.amount;
    } else if (this.type == SystemInstructionEnum.CREATE) {
      return data.lamports;
    }
    return null;
  }
}

/**
 * @typedef {Object} SystemInstructionType
 * @property (index} The System Instruction index (from solana-sdk)
 * @property (BufferLayout} The BufferLayout to use to build data
 */
type SystemInstructionType = {|
  index: number,
  layout: typeof BufferLayout,
|};

/**
 * An enumeration of valid SystemInstructionTypes
 */
const SystemInstructionEnum = Object.freeze({
  CREATE: {
    index: 0,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      BufferLayout.ns64('lamports'),
      BufferLayout.ns64('space'),
      Layout.publicKey('programId'),
    ]),
  },
  ASSIGN: {
    index: 1,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      Layout.publicKey('programId'),
    ]),
  },
  TRANSFER: {
    index: 2,
    layout: BufferLayout.struct([
      BufferLayout.u32('instruction'),
      BufferLayout.ns64('amount'),
    ]),
  },
});

/**
 * Populate a buffer of instruction data using the SystemInstructionType
 */
function encodeData(type: SystemInstructionType, fields: Object): Buffer {
  const data = Buffer.alloc(type.layout.span);
  const layoutFields = Object.assign({instruction: type.index}, fields);
  type.layout.encode(layoutFields, data);
  return data;
}

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
   * Generate a Transaction that creates a new account
   */
  static createAccount(
    from: PublicKey,
    newAccount: PublicKey,
    lamports: number,
    space: number,
    programId: PublicKey,
  ): Transaction {
    const type = SystemInstructionEnum.CREATE;
    const data = encodeData(type, {
      lamports,
      space,
      programId: programId.toBuffer(),
    });

    return new Transaction().add({
      keys: [
        {pubkey: from, isSigner: true, isDebitable: true},
        {pubkey: newAccount, isSigner: false, isDebitable: true},
      ],
      programId: SystemProgram.programId,
      data,
    });
  }

  /**
   * Generate a Transaction that transfers lamports from one account to another
   */
  static transfer(from: PublicKey, to: PublicKey, amount: number): Transaction {
    const type = SystemInstructionEnum.TRANSFER;
    const data = encodeData(type, {amount});

    return new Transaction().add({
      keys: [
        {pubkey: from, isSigner: true, isDebitable: true},
        {pubkey: to, isSigner: false, isDebitable: false},
      ],
      programId: SystemProgram.programId,
      data,
    });
  }

  /**
   * Generate a Transaction that assigns an account to a program
   */
  static assign(from: PublicKey, programId: PublicKey): Transaction {
    const type = SystemInstructionEnum.ASSIGN;
    const data = encodeData(type, {programId: programId.toBuffer()});

    return new Transaction().add({
      keys: [{pubkey: from, isSigner: true, isDebitable: true}],
      programId: SystemProgram.programId,
      data,
    });
  }
}
