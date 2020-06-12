import React from "react";
import { Link } from "react-router-dom";

import ClusterStatusButton from "./components/ClusterStatusButton";
import AccountsCard from "./components/AccountsCard";
import TransactionsCard from "./components/TransactionsCard";
import ClusterModal from "./components/ClusterModal";
import TransactionModal from "./components/TransactionModal";
import AccountModal from "./components/AccountModal";
import Logo from "./img/logos-solana/light-explorer-logo.svg";
import { useCurrentTab, Tab } from "./providers/tab";

function App() {
  const [showClusterModal, setShowClusterModal] = React.useState(false);
  const currentTab = useCurrentTab();
  return (
    <>
      <ClusterModal
        show={showClusterModal}
        onClose={() => setShowClusterModal(false)}
      />
      <TransactionModal />
      <AccountModal />
      <div className="main-content">
        <nav className="navbar navbar-expand-xl navbar-light">
          <div className="container">
            <div className="row align-items-end">
              <div className="col">
                <img src={Logo} width="250" alt="Solana Explorer" />
              </div>
            </div>
          </div>
        </nav>

        <div className="header">
          <div className="container">
            <div className="header-body">
              <div className="row align-items-center d-md-none">
                <div className="col-12">
                  <ClusterStatusButton
                    expand
                    onClick={() => setShowClusterModal(true)}
                  />
                </div>
              </div>
              <div className="row align-items-center">
                <div className="col">
                  <ul className="nav nav-tabs nav-overflow header-tabs">
                    <li className="nav-item">
                      <NavLink href="/transactions" tab="Transactions" />
                    </li>
                    <li className="nav-item">
                      <NavLink href="/accounts" tab="Accounts" />
                    </li>
                  </ul>
                </div>
                <div className="col-auto d-none d-md-block">
                  <ClusterStatusButton
                    onClick={() => setShowClusterModal(true)}
                  />
                </div>
              </div>
            </div>
          </div>
        </div>

        <div className="container">
          <div className="row">
            <div className="col-12">
              {currentTab === "Transactions" ? <TransactionsCard /> : null}
            </div>
          </div>
          <div className="row">
            <div className="col-12">
              {currentTab === "Accounts" ? <AccountsCard /> : null}
            </div>
          </div>
        </div>
      </div>
    </>
  );
}

function NavLink({ href, tab }: { href: string; tab: Tab }) {
  let classes = "nav-link";
  if (tab === useCurrentTab()) {
    classes += " active";
  }

  return (
    <Link to={href} className={classes}>
      {tab}
    </Link>
  );
}

export default App;
