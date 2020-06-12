import {
  PublicKey,
  SystemProgram,
  StakeProgram,
  VOTE_PROGRAM_ID,
  BpfLoader,
  TransferParams,
  SystemInstruction,
  CreateAccountParams,
  TransactionInstruction,
  SYSVAR_CLOCK_PUBKEY,
  SYSVAR_RENT_PUBKEY,
  SYSVAR_REWARDS_PUBKEY,
  SYSVAR_STAKE_HISTORY_PUBKEY
} from "@solana/web3.js";

const PROGRAM_IDS = {
  Budget1111111111111111111111111111111111111: "Budget",
  Config1111111111111111111111111111111111111: "Config",
  Exchange11111111111111111111111111111111111: "Exchange",
  [StakeProgram.programId.toBase58()]: "Stake",
  Storage111111111111111111111111111111111111: "Storage",
  [SystemProgram.programId.toBase58()]: "System",
  Vest111111111111111111111111111111111111111: "Vest",
  [VOTE_PROGRAM_ID.toBase58()]: "Vote"
};

const LOADER_IDS = {
  MoveLdr111111111111111111111111111111111111: "Move Loader",
  NativeLoader1111111111111111111111111111111: "Native Loader",
  [BpfLoader.programId.toBase58()]: "BPF Loader"
};

const SYSVAR_IDS = {
  Sysvar1111111111111111111111111111111111111: "SYSVAR",
  [SYSVAR_CLOCK_PUBKEY.toBase58()]: "SYSVAR_CLOCK",
  SysvarEpochSchedu1e111111111111111111111111: "SYSVAR_EPOCH_SCHEDULE",
  SysvarFees111111111111111111111111111111111: "SYSVAR_FEES",
  SysvarRecentB1ockHashes11111111111111111111: "SYSVAR_RECENT_BLOCKHASHES",
  [SYSVAR_RENT_PUBKEY.toBase58()]: "SYSVAR_RENT",
  [SYSVAR_REWARDS_PUBKEY.toBase58()]: "SYSVAR_REWARDS",
  SysvarS1otHashes111111111111111111111111111: "SYSVAR_SLOT_HASHES",
  SysvarS1otHistory11111111111111111111111111: "SYSVAR_SLOT_HISTORY",
  [SYSVAR_STAKE_HISTORY_PUBKEY.toBase58()]: "SYSVAR_STAKE_HISTORY"
};

export function displayAddress(pubkey: PublicKey): string {
  const address = pubkey.toBase58();
  return (
    PROGRAM_IDS[address] ||
    LOADER_IDS[address] ||
    SYSVAR_IDS[address] ||
    address
  );
}

export function decodeTransfer(
  ix: TransactionInstruction
): TransferParams | null {
  if (!ix.programId.equals(SystemProgram.programId)) return null;

  try {
    if (SystemInstruction.decodeInstructionType(ix) !== "Transfer") return null;
    return SystemInstruction.decodeTransfer(ix);
  } catch (err) {
    console.error(ix, err);
    return null;
  }
}

export function decodeCreate(
  ix: TransactionInstruction
): CreateAccountParams | null {
  if (!ix.programId.equals(SystemProgram.programId)) return null;

  try {
    if (SystemInstruction.decodeInstructionType(ix) !== "Create") return null;
    return SystemInstruction.decodeCreateAccount(ix);
  } catch (err) {
    console.error(ix, err);
    return null;
  }
}
