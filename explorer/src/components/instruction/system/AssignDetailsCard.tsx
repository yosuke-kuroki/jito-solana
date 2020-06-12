import React from "react";
import {
  TransactionInstruction,
  SystemProgram,
  SignatureResult,
  SystemInstruction
} from "@solana/web3.js";
import { displayAddress } from "utils/tx";
import { InstructionCard } from "../InstructionCard";
import Copyable from "components/Copyable";
import { UnknownDetailsCard } from "../UnknownDetailsCard";

export function AssignDetailsCard(props: {
  ix: TransactionInstruction;
  index: number;
  result: SignatureResult;
}) {
  const { ix, index, result } = props;

  let params;
  try {
    params = SystemInstruction.decodeAssign(ix);
  } catch (err) {
    console.error(err);
    return <UnknownDetailsCard {...props} />;
  }

  const from = params.fromPubkey.toBase58();

  return (
    <InstructionCard
      ix={ix}
      index={index}
      result={result}
      title="Assign Account"
    >
      <tr>
        <td>Program</td>
        <td className="text-right">
          <Copyable bottom text={SystemProgram.programId.toBase58()}>
            <code>{displayAddress(SystemProgram.programId)}</code>
          </Copyable>
        </td>
      </tr>

      <tr>
        <td>From Address</td>
        <td className="text-right">
          <Copyable text={from}>
            <code>{from}</code>
          </Copyable>
        </td>
      </tr>

      <tr>
        <td>Assigned Owner</td>
        <td className="text-right">
          <Copyable text={params.programId.toBase58()}>
            <code>{displayAddress(params.programId)}</code>
          </Copyable>
        </td>
      </tr>
    </InstructionCard>
  );
}
