import React from "react";
import {
  Source,
  useFetchTransactionStatus,
  useTransactionStatus,
  useTransactionDetails,
  useDetailsDispatch,
  FetchStatus
} from "../providers/transactions";
import { fetchDetails } from "providers/transactions/details";
import { useCluster, useClusterModal } from "providers/cluster";
import {
  TransactionSignature,
  SystemInstruction,
  SystemProgram
} from "@solana/web3.js";
import ClusterStatusButton from "components/ClusterStatusButton";
import { lamportsToSolString } from "utils";
import { displayAddress } from "utils/tx";
import Copyable from "./Copyable";
import { useHistory, useLocation } from "react-router-dom";
import { TransferDetailsCard } from "./instruction/TransferDetailsCard";
import { AssignDetailsCard } from "./instruction/AssignDetailsCard";
import { CreateDetailsCard } from "./instruction/CreateDetailsCard";
import { CreateWithSeedDetailsCard } from "./instruction/CreateWithSeedDetailsCard";
import { UnknownDetailsCard } from "./instruction/UnknownDetailsCard";
import { NonceInitializeDetailsCard } from "./instruction/NonceInitializeDetailsCard";
import { NonceAdvanceDetailsCard } from "./instruction/NonceAdvanceDetailsCard";
import { NonceWithdrawDetailsCard } from "./instruction/NonceWithdrawDetailsCard";
import { NonceAuthorizeDetailsCard } from "./instruction/NonceAuthorizeDetailsCard";

type Props = { signature: TransactionSignature };
export default function TransactionDetails({ signature }: Props) {
  const fetchTransaction = useFetchTransactionStatus();
  const [, setShow] = useClusterModal();
  const [search, setSearch] = React.useState(signature);
  const history = useHistory();
  const location = useLocation();

  const updateSignature = () => {
    history.push({ ...location, pathname: "/tx/" + search });
  };

  // Fetch transaction on load
  React.useEffect(() => {
    fetchTransaction(signature, Source.Url);
  }, [signature]); // eslint-disable-line react-hooks/exhaustive-deps

  const searchInput = (
    <input
      type="text"
      value={search}
      onChange={e => setSearch(e.target.value)}
      onKeyUp={e => e.key === "Enter" && updateSignature()}
      className="form-control form-control-prepended search text-monospace"
      placeholder="Search for signature"
    />
  );

  return (
    <div className="container">
      <div className="header">
        <div className="header-body">
          <div className="row align-items-center">
            <div className="col">
              <h6 className="header-pretitle">Details</h6>
              <h3 className="header-title">Transaction</h3>
            </div>
            <div className="col-auto">
              <ClusterStatusButton onClick={() => setShow(true)} />
            </div>
          </div>
        </div>
      </div>

      <div className="row mb-4 mt-n2 align-items-center">
        <div className="col d-none d-md-block">
          <div className="input-group input-group-merge">
            {searchInput}
            <div className="input-group-prepend">
              <div className="input-group-text">
                <span className="fe fe-search"></span>
              </div>
            </div>
          </div>
        </div>
        <div className="col d-block d-md-none">{searchInput}</div>
        <div className="col-auto ml-n3 d-block d-md-none">
          <button className="btn btn-white" onClick={updateSignature}>
            <span className="fe fe-search"></span>
          </button>
        </div>
      </div>

      <StatusCard signature={signature} />
      <AccountsCard signature={signature} />
      <InstructionsSection signature={signature} />
    </div>
  );
}

function StatusCard({ signature }: Props) {
  const status = useTransactionStatus(signature);
  const refresh = useFetchTransactionStatus();
  const details = useTransactionDetails(signature);

  if (!status || status.fetchStatus === FetchStatus.Fetching) {
    return <LoadingCard />;
  } else if (status?.fetchStatus === FetchStatus.FetchFailed) {
    return <RetryCard retry={() => refresh(signature)} text="Fetch Failed" />;
  } else if (!status.info) {
    return <RetryCard retry={() => refresh(signature)} text="Not Found" />;
  }

  const { info } = status;
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

  const fee = details?.transaction?.meta?.fee;
  const blockhash = details?.transaction?.transaction.recentBlockhash;
  return (
    <div className="card">
      <div className="card-header align-items-center">
        <h3 className="card-header-title">Status</h3>
        <button
          className="btn btn-white btn-sm"
          onClick={() => refresh(signature)}
        >
          <span className="fe fe-refresh-cw mr-2"></span>
          Refresh
        </button>
      </div>

      <TableCardBody>
        <tr>
          <td>Result</td>
          <td className="text-right">{renderResult()}</td>
        </tr>

        <tr>
          <td>Confirmations</td>
          <td className="text-right text-uppercase">{info.confirmations}</td>
        </tr>

        <tr>
          <td>Block</td>
          <td className="text-right">{info.slot}</td>
        </tr>

        {blockhash && (
          <tr>
            <td>Blockhash</td>
            <td className="text-right">
              <code>{blockhash}</code>
            </td>
          </tr>
        )}

        {fee && (
          <tr>
            <td>Fee (SOL)</td>
            <td className="text-right">{lamportsToSolString(fee)}</td>
          </tr>
        )}
      </TableCardBody>
    </div>
  );
}

function AccountsCard({ signature }: Props) {
  const details = useTransactionDetails(signature);
  const dispatch = useDetailsDispatch();
  const { url } = useCluster();

  const fetchStatus = useFetchTransactionStatus();
  const refreshStatus = () => fetchStatus(signature);
  const refreshDetails = () => fetchDetails(dispatch, signature, url);
  const transaction = details?.transaction?.transaction;
  const message = React.useMemo(() => {
    return transaction?.compileMessage();
  }, [transaction]);

  const status = useTransactionStatus(signature);

  if (!status || !status.info) {
    return null;
  } else if (!details) {
    return (
      <RetryCard
        retry={refreshStatus}
        text="Details are not available until the transaction reaches MAX confirmations"
      />
    );
  } else if (details.fetchStatus === FetchStatus.Fetching) {
    return <LoadingCard />;
  } else if (details?.fetchStatus === FetchStatus.FetchFailed) {
    return <RetryCard retry={refreshDetails} text="Fetch Failed" />;
  } else if (!details.transaction || !message) {
    return <RetryCard retry={refreshDetails} text="Not Found" />;
  }

  const { meta } = details.transaction;
  if (!meta) {
    return <RetryCard retry={refreshDetails} text="Metadata Missing" />;
  }

  const accountRows = message.accountKeys.map((pubkey, index) => {
    const pre = meta.preBalances[index];
    const post = meta.postBalances[index];
    const key = pubkey.toBase58();
    const renderChange = () => {
      const change = post - pre;
      if (change === 0) return "";
      const sols = lamportsToSolString(change);
      if (change > 0) {
        return <span className="badge badge-soft-success">{"+" + sols}</span>;
      } else {
        return <span className="badge badge-soft-warning">{"-" + sols}</span>;
      }
    };

    return (
      <tr key={key}>
        <td>
          <Copyable text={key}>
            <code>{displayAddress(pubkey)}</code>
          </Copyable>
        </td>
        <td>{renderChange()}</td>
        <td>{lamportsToSolString(post)}</td>
        <td>
          {index === 0 && (
            <span className="badge badge-soft-dark mr-1">Fee Payer</span>
          )}
          {!message.isAccountWritable(index) && (
            <span className="badge badge-soft-dark mr-1">Readonly</span>
          )}
          {index < message.header.numRequiredSignatures && (
            <span className="badge badge-soft-dark mr-1">Signer</span>
          )}
          {message.instructions.find(ix => ix.programIdIndex === index) && (
            <span className="badge badge-soft-dark mr-1">Program</span>
          )}
        </td>
      </tr>
    );
  });

  return (
    <div className="card">
      <div className="card-header">
        <h3 className="card-header-title">Accounts</h3>
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

function InstructionsSection({ signature }: Props) {
  const status = useTransactionStatus(signature);
  const details = useTransactionDetails(signature);
  const dispatch = useDetailsDispatch();
  const { url } = useCluster();
  const refreshDetails = () => fetchDetails(dispatch, signature, url);

  if (!status || !status.info || !details || !details.transaction) return null;

  const { transaction } = details.transaction;
  if (transaction.instructions.length === 0) {
    return <RetryCard retry={refreshDetails} text="No instructions found" />;
  }

  const result = status.info.result;
  const instructionDetails = transaction.instructions.map((ix, index) => {
    const props = { ix, result, index };

    if (!ix.programId.equals(SystemProgram.programId)) {
      return <UnknownDetailsCard key={index} {...props} />;
    }

    let systemInstructionType;
    try {
      systemInstructionType = SystemInstruction.decodeInstructionType(ix);
    } catch (err) {
      console.error(err);
      return <UnknownDetailsCard key={index} {...props} />;
    }

    switch (systemInstructionType) {
      case "Create":
        return <CreateDetailsCard key={index} {...props} />;
      case "Assign":
        return <AssignDetailsCard key={index} {...props} />;
      case "Transfer":
        return <TransferDetailsCard key={index} {...props} />;
      case "CreateWithSeed":
        return <CreateWithSeedDetailsCard key={index} {...props} />;
      case "AdvanceNonceAccount":
        return <NonceAdvanceDetailsCard key={index} {...props} />;
      case "WithdrawNonceAccount":
        return <NonceWithdrawDetailsCard key={index} {...props} />;
      case "AuthorizeNonceAccount":
        return <NonceAuthorizeDetailsCard key={index} {...props} />;
      case "InitializeNonceAccount":
        return <NonceInitializeDetailsCard key={index} {...props} />;
      default:
        return <UnknownDetailsCard key={index} {...props} />;
    }
  });

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

function LoadingCard() {
  return (
    <div className="card">
      <div className="card-body text-center">
        <span className="spinner-grow spinner-grow-sm mr-2"></span>
        Loading
      </div>
    </div>
  );
}

function RetryCard({ retry, text }: { retry: () => void; text: string }) {
  return (
    <div className="card">
      <div className="card-body text-center">
        {text}
        <span className="btn btn-white ml-3 d-none d-md-inline" onClick={retry}>
          Try Again
        </span>
        <div className="d-block d-md-none mt-4">
          <hr></hr>
          <span className="btn btn-white" onClick={retry}>
            Try Again
          </span>
        </div>
      </div>
    </div>
  );
}

function TableCardBody({ children }: { children: React.ReactNode }) {
  return (
    <div className="table-responsive mb-0">
      <table className="table table-sm table-nowrap card-table">
        <tbody className="list">{children}</tbody>
      </table>
    </div>
  );
}
