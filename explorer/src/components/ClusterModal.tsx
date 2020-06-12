import React from "react";
import { Link, useLocation, useHistory } from "react-router-dom";
import { Location } from "history";
import {
  useCluster,
  ClusterStatus,
  clusterUrl,
  clusterName,
  clusterSlug,
  CLUSTERS,
  Cluster,
  useClusterModal
} from "../providers/cluster";
import { assertUnreachable } from "../utils";
import Overlay from "./Overlay";

function ClusterModal() {
  const [show, setShow] = useClusterModal();
  const onClose = () => setShow(false);
  return (
    <>
      <div
        className={`modal fade fixed-right${show ? " show" : ""}`}
        onClick={onClose}
      >
        <div className="modal-dialog modal-dialog-vertical">
          <div className="modal-content">
            <div className="modal-body" onClick={e => e.stopPropagation()}>
              <span className="close" onClick={onClose}>
                &times;
              </span>

              <h2 className="text-center mb-4 mt-4">Choose a Cluster</h2>

              <ClusterToggle />
            </div>
          </div>
        </div>
      </div>

      <Overlay show={show} />
    </>
  );
}

type InputProps = { activeSuffix: string; active: boolean };
function CustomClusterInput({ activeSuffix, active }: InputProps) {
  const { customUrl } = useCluster();
  const [editing, setEditing] = React.useState(false);
  const history = useHistory();
  const location = useLocation();

  const customClass = (prefix: string) =>
    active ? `${prefix}-${activeSuffix}` : "";

  const clusterLocation = (location: Location, url: string) => {
    const params = new URLSearchParams(location.search);
    params.set("clusterUrl", url);
    params.delete("cluster");
    return {
      ...location,
      search: params.toString()
    };
  };

  const updateCustomUrl = React.useCallback(
    (url: string) => {
      history.push(clusterLocation(location, url));
    },
    [history, location]
  );

  const inputTextClass = editing ? "" : "text-muted";
  return (
    <Link
      to={location => clusterLocation(location, customUrl)}
      className="btn input-group input-group-merge p-0"
    >
      <input
        type="text"
        defaultValue={customUrl}
        className={`form-control form-control-prepended ${inputTextClass} ${customClass(
          "border"
        )}`}
        onFocus={() => setEditing(true)}
        onBlur={() => setEditing(false)}
        onInput={e => updateCustomUrl(e.currentTarget.value)}
      />
      <div className="input-group-prepend">
        <div className={`input-group-text pr-0 ${customClass("border")}`}>
          <span className={customClass("text") || "text-dark"}>Custom:</span>
        </div>
      </div>
    </Link>
  );
}

function ClusterToggle() {
  const { status, cluster, customUrl } = useCluster();

  let activeSuffix = "";
  switch (status) {
    case ClusterStatus.Connected:
      activeSuffix = "success";
      break;
    case ClusterStatus.Connecting:
      activeSuffix = "warning";
      break;
    case ClusterStatus.Failure:
      activeSuffix = "danger";
      break;
    default:
      assertUnreachable(status);
  }

  return (
    <div className="btn-group-toggle d-flex flex-wrap mb-4">
      {CLUSTERS.map((net, index) => {
        const active = net === cluster;
        if (net === Cluster.Custom)
          return (
            <CustomClusterInput
              key={index}
              activeSuffix={activeSuffix}
              active={active}
            />
          );

        const btnClass = active
          ? `border-${activeSuffix} text-${activeSuffix}`
          : "btn-white text-dark";

        const clusterLocation = (location: Location) => {
          const params = new URLSearchParams(location.search);
          const slug = clusterSlug(net);
          if (slug && slug !== "mainnet-beta") {
            params.set("cluster", slug);
            params.delete("clusterUrl");
          } else {
            params.delete("cluster");
            if (slug === "mainnet-beta") {
              params.delete("clusterUrl");
            }
          }
          return {
            ...location,
            search: params.toString()
          };
        };

        return (
          <Link
            key={index}
            className={`btn text-left col-12 mb-3 ${btnClass}`}
            to={clusterLocation}
          >
            {`${clusterName(net)}: `}
            <span className="text-muted d-inline-block">
              {clusterUrl(net, customUrl)}
            </span>
          </Link>
        );
      })}
    </div>
  );
}

export default ClusterModal;
