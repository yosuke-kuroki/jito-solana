import React from "react";
import {
  useTransactions,
  useTransactionsDispatch,
  checkTransactionStatus,
  ActionType,
  Transaction,
  Status
} from "../providers/transactions";
import bs58 from "bs58";
import { assertUnreachable } from "../utils";
import { useNetwork } from "../providers/network";

function TransactionsCard() {
  const { transactions, idCounter } = useTransactions();
  const dispatch = useTransactionsDispatch();
  const signatureInput = React.useRef<HTMLInputElement>(null);
  const [error, setError] = React.useState("");
  const { url } = useNetwork();

  const onNew = (signature: string) => {
    if (signature.length === 0) return;
    try {
      const length = bs58.decode(signature).length;
      if (length > 64) {
        setError("Signature is too short");
        return;
      } else if (length < 64) {
        setError("Signature is too short");
        return;
      }
    } catch (err) {
      setError(`${err}`);
      return;
    }

    dispatch({ type: ActionType.InputSignature, signature });
    checkTransactionStatus(dispatch, idCounter + 1, signature, url);

    const inputEl = signatureInput.current;
    if (inputEl) {
      inputEl.value = "";
    }
  };

  return (
    <div className="card">
      {renderHeader()}

      <div className="table-responsive mb-0">
        <table className="table table-sm table-nowrap card-table">
          <thead>
            <tr>
              <th className="text-muted">
                <span className="fe fe-hash"></span>
              </th>
              <th className="text-muted">Status</th>
              <th className="text-muted">Signature</th>
              <th className="text-muted">Confirmations</th>
              <th className="text-muted">Slot Number</th>
            </tr>
          </thead>
          <tbody className="list">
            <tr>
              <td>
                <span className="badge badge-soft-dark badge-pill">
                  {idCounter + 1}
                </span>
              </td>

              <td>
                <span className={`badge badge-soft-primary`}>New</span>
              </td>
              <td>
                <input
                  type="text"
                  onInput={() => setError("")}
                  onKeyDown={e =>
                    e.keyCode === 13 && onNew(e.currentTarget.value)
                  }
                  onSubmit={e => onNew(e.currentTarget.value)}
                  ref={signatureInput}
                  className={`form-control text-signature text-monospace ${
                    error ? "is-invalid" : ""
                  }`}
                  placeholder="abcd..."
                />
                {error ? <div className="invalid-feedback">{error}</div> : null}
              </td>
              <td>-</td>
              <td>-</td>
            </tr>
            {transactions.map(transaction => renderTransactionRow(transaction))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

const renderHeader = () => {
  return (
    <div className="card-header">
      <div className="row align-items-center">
        <div className="col">
          <h4 className="card-header-title">Transactions</h4>
        </div>
      </div>
    </div>
  );
};

const renderTransactionRow = (transaction: Transaction) => {
  let statusText;
  let statusClass;
  switch (transaction.status) {
    case Status.CheckFailed:
      statusClass = "dark";
      statusText = "Network Error";
      break;
    case Status.Checking:
      statusClass = "info";
      statusText = "Checking";
      break;
    case Status.Success:
      statusClass = "success";
      statusText = "Success";
      break;
    case Status.Failure:
      statusClass = "danger";
      statusText = "Failed";
      break;
    case Status.Missing:
      statusClass = "warning";
      statusText = "Not Found";
      break;
    default:
      return assertUnreachable(transaction.status);
  }

  return (
    <tr key={transaction.signature}>
      <td>
        <span className="badge badge-soft-dark badge-pill">
          {transaction.id}
        </span>
      </td>
      <td>
        <span className={`badge badge-soft-${statusClass}`}>{statusText}</span>
      </td>
      <td>
        <code>{transaction.signature}</code>
      </td>
      <td>-</td>
      <td>-</td>
    </tr>
  );
};

export default TransactionsCard;
