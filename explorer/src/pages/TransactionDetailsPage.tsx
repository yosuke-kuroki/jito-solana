import React from "react";
import bs58 from "bs58";
import {
  useFetchTransactionStatus,
  useTransactionStatus,
  useTransactionDetails,
} from "providers/transactions";
import { useFetchTransactionDetails } from "providers/transactions/details";
import { useCluster, ClusterStatus, Cluster } from "providers/cluster";
import {
  TransactionSignature,
  SystemProgram,
  SystemInstruction,
  ParsedInstruction,
  PartiallyDecodedInstruction,
  SignatureResult,
  ParsedTransaction,
  ParsedInnerInstruction,
  Transaction,
} from "@solana/web3.js";
import { PublicKey } from "@solana/web3.js";
import { lamportsToSolString } from "utils";
import { UnknownDetailsCard } from "components/instruction/UnknownDetailsCard";
import { SystemDetailsCard } from "components/instruction/system/SystemDetailsCard";
import { StakeDetailsCard } from "components/instruction/stake/StakeDetailsCard";
import { BpfLoaderDetailsCard } from "components/instruction/bpf-loader/BpfLoaderDetailsCard";
import { ErrorCard } from "components/common/ErrorCard";
import { LoadingCard } from "components/common/LoadingCard";
import { TableCardBody } from "components/common/TableCardBody";
import { displayTimestamp } from "utils/date";
import { InfoTooltip } from "components/common/InfoTooltip";
import { Address } from "components/common/Address";
import { Signature } from "components/common/Signature";
import { intoTransactionInstruction } from "utils/tx";
import { TokenDetailsCard } from "components/instruction/token/TokenDetailsCard";
import { FetchStatus } from "providers/cache";
import { SerumDetailsCard } from "components/instruction/SerumDetailsCard";
import { Slot } from "components/common/Slot";
import { isTokenSwapInstruction } from "components/instruction/token-swap/types";
import { isTokenLendingInstruction } from "components/instruction/token-lending/types";
import { TokenSwapDetailsCard } from "components/instruction/TokenSwapDetailsCard";
import { TokenLendingDetailsCard } from "components/instruction/TokenLendingDetailsCard";
import { isSerumInstruction } from "components/instruction/serum/types";
import { MemoDetailsCard } from "components/instruction/MemoDetailsCard";
import { BigNumber } from "bignumber.js";
import { BalanceDelta } from "components/common/BalanceDelta";
import { TokenBalancesCard } from "components/transaction/TokenBalancesCard";

const AUTO_REFRESH_INTERVAL = 2000;
const ZERO_CONFIRMATION_BAILOUT = 5;
export const INNER_INSTRUCTIONS_START_SLOT = 46915769;

export type SignatureProps = {
  signature: TransactionSignature;
};

export const SignatureContext = React.createContext("");

enum AutoRefresh {
  Active,
  Inactive,
  BailedOut,
}

type AutoRefreshProps = {
  autoRefresh: AutoRefresh;
};

export function TransactionDetailsPage({ signature: raw }: SignatureProps) {
  let signature: TransactionSignature | undefined;

  try {
    const decoded = bs58.decode(raw);
    if (decoded.length === 64) {
      signature = raw;
    }
  } catch (err) {}

  const status = useTransactionStatus(signature);
  const [zeroConfirmationRetries, setZeroConfirmationRetries] = React.useState(
    0
  );

  let autoRefresh = AutoRefresh.Inactive;

  if (zeroConfirmationRetries >= ZERO_CONFIRMATION_BAILOUT) {
    autoRefresh = AutoRefresh.BailedOut;
  } else if (status?.data?.info && status.data.info.confirmations !== "max") {
    autoRefresh = AutoRefresh.Active;
  }

  React.useEffect(() => {
    if (
      status?.status === FetchStatus.Fetched &&
      status.data?.info &&
      status.data.info.confirmations === 0
    ) {
      setZeroConfirmationRetries((retries) => retries + 1);
    }
  }, [status]);

  React.useEffect(() => {
    if (
      status?.status === FetchStatus.Fetching &&
      autoRefresh === AutoRefresh.BailedOut
    ) {
      setZeroConfirmationRetries(0);
    }
  }, [status, autoRefresh, setZeroConfirmationRetries]);

  return (
    <div className="container mt-n3">
      <div className="header">
        <div className="header-body">
          <h6 className="header-pretitle">Details</h6>
          <h2 className="header-title">Transaction</h2>
        </div>
      </div>
      {signature === undefined ? (
        <ErrorCard text={`Signature "${raw}" is not valid`} />
      ) : (
        <>
          <StatusCard signature={signature} autoRefresh={autoRefresh} />
          <AccountsCard signature={signature} autoRefresh={autoRefresh} />
          <TokenBalancesCard signature={signature} />
          <SignatureContext.Provider value={signature}>
            <InstructionsSection signature={signature} />
          </SignatureContext.Provider>
          <ProgramLogSection signature={signature} />
        </>
      )}
    </div>
  );
}

function StatusCard({
  signature,
  autoRefresh,
}: SignatureProps & AutoRefreshProps) {
  const fetchStatus = useFetchTransactionStatus();
  const status = useTransactionStatus(signature);
  const details = useTransactionDetails(signature);
  const { firstAvailableBlock, status: clusterStatus } = useCluster();

  // Fetch transaction on load
  React.useEffect(() => {
    if (!status && clusterStatus === ClusterStatus.Connected) {
      fetchStatus(signature);
    }
  }, [signature, clusterStatus]); // eslint-disable-line react-hooks/exhaustive-deps

  // Effect to set and clear interval for auto-refresh
  React.useEffect(() => {
    if (autoRefresh === AutoRefresh.Active) {
      let intervalHandle: NodeJS.Timeout = setInterval(
        () => fetchStatus(signature),
        AUTO_REFRESH_INTERVAL
      );

      return () => {
        clearInterval(intervalHandle);
      };
    }
  }, [autoRefresh, fetchStatus, signature]);

  if (
    !status ||
    (status.status === FetchStatus.Fetching &&
      autoRefresh === AutoRefresh.Inactive)
  ) {
    return <LoadingCard />;
  } else if (status.status === FetchStatus.FetchFailed) {
    return (
      <ErrorCard retry={() => fetchStatus(signature)} text="Fetch Failed" />
    );
  } else if (!status.data?.info) {
    if (firstAvailableBlock !== undefined && firstAvailableBlock > 1) {
      return (
        <ErrorCard
          retry={() => fetchStatus(signature)}
          text="Not Found"
          subtext={`Note: Transactions processed before block ${firstAvailableBlock} are not available at this time`}
        />
      );
    }
    return <ErrorCard retry={() => fetchStatus(signature)} text="Not Found" />;
  }

  const { info } = status.data;

  const renderResult = () => {
    let statusClass = "success";
    let statusText = "Success";
    if (info.result.err) {
      statusClass = "warning";
      statusText = "Error";
    }

    return (
      <h3 className="mb-0">
        <span className={`badge badge-soft-${statusClass}`}>{statusText}</span>
      </h3>
    );
  };

  const fee = details?.data?.transaction?.meta?.fee;
  const transaction = details?.data?.transaction?.transaction;
  const blockhash = transaction?.message.recentBlockhash;
  const isNonce = (() => {
    if (!transaction || transaction.message.instructions.length < 1) {
      return false;
    }

    const ix = intoTransactionInstruction(
      transaction,
      transaction.message.instructions[0]
    );
    return (
      ix &&
      SystemProgram.programId.equals(ix.programId) &&
      SystemInstruction.decodeInstructionType(ix) === "AdvanceNonceAccount"
    );
  })();

  return (
    <div className="card">
      <div className="card-header align-items-center">
        <h3 className="card-header-title">Overview</h3>
        {autoRefresh === AutoRefresh.Active ? (
          <span className="spinner-grow spinner-grow-sm"></span>
        ) : (
          <button
            className="btn btn-white btn-sm"
            onClick={() => fetchStatus(signature)}
          >
            <span className="fe fe-refresh-cw mr-2"></span>
            Refresh
          </button>
        )}
      </div>

      <TableCardBody>
        <tr>
          <td>Signature</td>
          <td className="text-lg-right">
            <Signature signature={signature} alignRight />
          </td>
        </tr>

        <tr>
          <td>Result</td>
          <td className="text-lg-right">{renderResult()}</td>
        </tr>

        <tr>
          <td>Timestamp</td>
          <td className="text-lg-right">
            {info.timestamp !== "unavailable" ? (
              displayTimestamp(info.timestamp * 1000)
            ) : (
              <InfoTooltip
                bottom
                right
                text="Timestamps are available for confirmed blocks within the past 5 epochs"
              >
                Unavailable
              </InfoTooltip>
            )}
          </td>
        </tr>

        <tr>
          <td>Confirmation Status</td>
          <td className="text-lg-right text-uppercase">
            {info.confirmationStatus || "Unknown"}
          </td>
        </tr>

        <tr>
          <td>Confirmations</td>
          <td className="text-lg-right text-uppercase">{info.confirmations}</td>
        </tr>

        <tr>
          <td>Block</td>
          <td className="text-lg-right">
            <Slot slot={info.slot} link />
          </td>
        </tr>

        {blockhash && (
          <tr>
            <td>
              {isNonce ? (
                "Nonce"
              ) : (
                <InfoTooltip text="Transactions use a previously confirmed blockhash as a nonce to prevent double spends">
                  Recent Blockhash
                </InfoTooltip>
              )}
            </td>
            <td className="text-lg-right">{blockhash}</td>
          </tr>
        )}

        {fee && (
          <tr>
            <td>Fee (SOL)</td>
            <td className="text-lg-right">{lamportsToSolString(fee)}</td>
          </tr>
        )}
      </TableCardBody>
    </div>
  );
}

function AccountsCard({
  signature,
  autoRefresh,
}: SignatureProps & AutoRefreshProps) {
  const details = useTransactionDetails(signature);
  const fetchDetails = useFetchTransactionDetails();
  const fetchStatus = useFetchTransactionStatus();
  const refreshDetails = () => fetchDetails(signature);
  const refreshStatus = () => fetchStatus(signature);
  const transaction = details?.data?.transaction?.transaction;
  const message = transaction?.message;
  const status = useTransactionStatus(signature);

  // Fetch details on load
  React.useEffect(() => {
    if (status?.data?.info?.confirmations === "max" && !details) {
      fetchDetails(signature);
    }
  }, [signature, details, status, fetchDetails]);

  if (!status?.data?.info) {
    return null;
  } else if (autoRefresh === AutoRefresh.BailedOut) {
    return (
      <ErrorCard
        text="Details are not available until the transaction reaches MAX confirmations"
        retry={refreshStatus}
      />
    );
  } else if (autoRefresh === AutoRefresh.Active) {
    return (
      <ErrorCard text="Details are not available until the transaction reaches MAX confirmations" />
    );
  } else if (!details || details.status === FetchStatus.Fetching) {
    return <LoadingCard />;
  } else if (details.status === FetchStatus.FetchFailed) {
    return <ErrorCard retry={refreshDetails} text="Failed to fetch details" />;
  } else if (!details.data?.transaction || !message) {
    return <ErrorCard text="Details are not available" />;
  }

  const { meta } = details.data.transaction;
  if (!meta) {
    return <ErrorCard text="Transaction metadata is missing" />;
  }

  const accountRows = message.accountKeys.map((account, index) => {
    const pre = meta.preBalances[index];
    const post = meta.postBalances[index];
    const pubkey = account.pubkey;
    const key = account.pubkey.toBase58();
    const delta = new BigNumber(post).minus(new BigNumber(pre));

    return (
      <tr key={key}>
        <td>
          <Address pubkey={pubkey} link />
        </td>
        <td>
          <BalanceDelta delta={delta} isSol />
        </td>
        <td>{lamportsToSolString(post)}</td>
        <td>
          {index === 0 && (
            <span className="badge badge-soft-info mr-1">Fee Payer</span>
          )}
          {!account.writable && (
            <span className="badge badge-soft-info mr-1">Readonly</span>
          )}
          {account.signer && (
            <span className="badge badge-soft-info mr-1">Signer</span>
          )}
          {message.instructions.find((ix) => ix.programId.equals(pubkey)) && (
            <span className="badge badge-soft-info mr-1">Program</span>
          )}
        </td>
      </tr>
    );
  });

  return (
    <div className="card">
      <div className="card-header">
        <h3 className="card-header-title">Account Inputs</h3>
      </div>
      <div className="table-responsive mb-0">
        <table className="table table-sm table-nowrap card-table">
          <thead>
            <tr>
              <th className="text-muted">Address</th>
              <th className="text-muted">Change (SOL)</th>
              <th className="text-muted">Post Balance (SOL)</th>
              <th className="text-muted">Details</th>
            </tr>
          </thead>
          <tbody className="list">{accountRows}</tbody>
        </table>
      </div>
    </div>
  );
}

function InstructionsSection({ signature }: SignatureProps) {
  const status = useTransactionStatus(signature);
  const details = useTransactionDetails(signature);
  const { cluster } = useCluster();
  const fetchDetails = useFetchTransactionDetails();
  const refreshDetails = () => fetchDetails(signature);

  if (!status?.data?.info || !details?.data?.transaction) return null;

  const raw = details.data.raw?.transaction;

  const { transaction } = details.data.transaction;
  const { meta } = details.data.transaction;

  if (transaction.message.instructions.length === 0) {
    return <ErrorCard retry={refreshDetails} text="No instructions found" />;
  }

  const innerInstructions: {
    [index: number]: (ParsedInstruction | PartiallyDecodedInstruction)[];
  } = {};

  if (
    meta?.innerInstructions &&
    (cluster !== Cluster.MainnetBeta ||
      details.data.transaction.slot >= INNER_INSTRUCTIONS_START_SLOT)
  ) {
    meta.innerInstructions.forEach((parsed: ParsedInnerInstruction) => {
      if (!innerInstructions[parsed.index]) {
        innerInstructions[parsed.index] = [];
      }

      parsed.instructions.forEach((ix) => {
        innerInstructions[parsed.index].push(ix);
      });
    });
  }

  const result = status.data.info.result;
  const instructionDetails = transaction.message.instructions.map(
    (instruction, index) => {
      let innerCards: JSX.Element[] = [];

      if (index in innerInstructions) {
        innerInstructions[index].forEach((ix, childIndex) => {
          if (typeof ix.programId === "string") {
            ix.programId = new PublicKey(ix.programId);
          }

          let res = renderInstructionCard({
            index,
            ix,
            result,
            signature,
            tx: transaction,
            childIndex,
            raw,
          });

          innerCards.push(res);
        });
      }

      return renderInstructionCard({
        index,
        ix: instruction,
        result,
        signature,
        tx: transaction,
        innerCards,
        raw,
      });
    }
  );

  return (
    <>
      <div className="container">
        <div className="header">
          <div className="header-body">
            <h3 className="mb-0">Instruction(s)</h3>
          </div>
        </div>
      </div>
      {instructionDetails}
    </>
  );
}

function ProgramLogSection({ signature }: SignatureProps) {
  const details = useTransactionDetails(signature);
  const logMessages = details?.data?.transaction?.meta?.logMessages;

  if (!logMessages || logMessages.length < 1) {
    return null;
  }

  return (
    <>
      <div className="container">
        <div className="header">
          <div className="header-body">
            <h3 className="card-header-title">Program Log</h3>
          </div>
        </div>
      </div>
      <div className="card">
        <ul className="log-messages">
          {logMessages.map((message, key) => (
            <li key={key}>{message.replace(/^Program log: /, "")}</li>
          ))}
        </ul>
      </div>
    </>
  );
}

function renderInstructionCard({
  ix,
  tx,
  result,
  index,
  signature,
  innerCards,
  childIndex,
  raw,
}: {
  ix: ParsedInstruction | PartiallyDecodedInstruction;
  tx: ParsedTransaction;
  result: SignatureResult;
  index: number;
  signature: TransactionSignature;
  innerCards?: JSX.Element[];
  childIndex?: number;
  raw?: Transaction;
}) {
  const key = `${index}-${childIndex}`;

  if ("parsed" in ix) {
    const props = {
      tx,
      ix,
      result,
      index,
      innerCards,
      childIndex,
      key,
    };

    switch (ix.program) {
      case "spl-token":
        return <TokenDetailsCard {...props} />;
      case "bpf-loader":
        return <BpfLoaderDetailsCard {...props} />;
      case "system":
        return <SystemDetailsCard {...props} />;
      case "stake":
        return <StakeDetailsCard {...props} />;
      case "spl-memo":
        return <MemoDetailsCard {...props} />;
      default:
        return <UnknownDetailsCard {...props} />;
    }
  }

  // TODO: There is a bug in web3, where inner instructions
  // aren't getting coerced. This is a temporary fix.

  if (typeof ix.programId === "string") {
    ix.programId = new PublicKey(ix.programId);
  }

  ix.accounts = ix.accounts.map((account) => {
    if (typeof account === "string") {
      return new PublicKey(account);
    }

    return account;
  });

  // TODO: End hotfix

  const transactionIx = intoTransactionInstruction(tx, ix);

  if (!transactionIx) {
    return (
      <ErrorCard
        key={key}
        text="Could not display this instruction, please report"
      />
    );
  }

  const props = {
    ix: transactionIx,
    result,
    index,
    signature,
    innerCards,
    childIndex,
  };

  if (isSerumInstruction(transactionIx)) {
    return <SerumDetailsCard key={key} {...props} />;
  } else if (isTokenSwapInstruction(transactionIx)) {
    return <TokenSwapDetailsCard key={key} {...props} />;
  } else if (isTokenLendingInstruction(transactionIx)) {
    return <TokenLendingDetailsCard key={key} {...props} />;
  } else {
    return <UnknownDetailsCard key={key} {...props} />;
  }
}
