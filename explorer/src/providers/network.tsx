import React from "react";
import { testnetChannelEndpoint, Connection } from "@solana/web3.js";
import { findGetParameter } from "../utils";

export enum NetworkStatus {
  Connected,
  Connecting,
  Failure
}

export enum Network {
  MainnetBeta,
  TdS,
  Devnet,
  Custom
}

export const NETWORKS = [
  Network.MainnetBeta,
  Network.TdS,
  Network.Devnet,
  Network.Custom
];

export function networkName(network: Network): string {
  switch (network) {
    case Network.MainnetBeta:
      return "Mainnet Beta";
    case Network.TdS:
      return "Tour de SOL";
    case Network.Devnet:
      return "Devnet";
    case Network.Custom:
      return "Custom";
  }
}

export const MAINNET_BETA_URL = "https://api.mainnet-beta.solana.com";
export const TDS_URL = "https://tds.solana.com";
export const DEVNET_URL = testnetChannelEndpoint("stable");

export const DEFAULT_NETWORK = Network.MainnetBeta;
export const DEFAULT_CUSTOM_URL = "http://localhost:8899";

interface State {
  network: Network;
  customUrl: string;
  status: NetworkStatus;
}

interface Connecting {
  status: NetworkStatus.Connecting;
  network: Network;
  customUrl: string;
}

interface Connected {
  status: NetworkStatus.Connected;
}

interface Failure {
  status: NetworkStatus.Failure;
}

type Action = Connected | Connecting | Failure;
type Dispatch = (action: Action) => void;

function networkReducer(state: State, action: Action): State {
  switch (action.status) {
    case NetworkStatus.Connected:
    case NetworkStatus.Failure: {
      return Object.assign({}, state, { status: action.status });
    }
    case NetworkStatus.Connecting: {
      return action;
    }
  }
}

function initState(): State {
  const networkUrlParam = findGetParameter("networkUrl");

  let network;
  let customUrl = DEFAULT_CUSTOM_URL;
  switch (networkUrlParam) {
    case null:
      network = DEFAULT_NETWORK;
      break;
    case MAINNET_BETA_URL:
      network = Network.MainnetBeta;
      break;
    case DEVNET_URL:
      network = Network.Devnet;
      break;
    case TDS_URL:
      network = Network.TdS;
      break;
    default:
      network = Network.Custom;
      customUrl = networkUrlParam || DEFAULT_CUSTOM_URL;
      break;
  }

  return {
    network,
    customUrl,
    status: NetworkStatus.Connecting
  };
}

const StateContext = React.createContext<State | undefined>(undefined);
const DispatchContext = React.createContext<Dispatch | undefined>(undefined);

type NetworkProviderProps = { children: React.ReactNode };
export function NetworkProvider({ children }: NetworkProviderProps) {
  const [state, dispatch] = React.useReducer(
    networkReducer,
    undefined,
    initState
  );

  React.useEffect(() => {
    // Connect to network immediately
    updateNetwork(dispatch, state.network, state.customUrl);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <StateContext.Provider value={state}>
      <DispatchContext.Provider value={dispatch}>
        {children}
      </DispatchContext.Provider>
    </StateContext.Provider>
  );
}

export function networkUrl(network: Network, customUrl: string) {
  switch (network) {
    case Network.Devnet:
      return DEVNET_URL;
    case Network.MainnetBeta:
      return MAINNET_BETA_URL;
    case Network.TdS:
      return TDS_URL;
    case Network.Custom:
      return customUrl;
  }
}

export async function updateNetwork(
  dispatch: Dispatch,
  network: Network,
  customUrl: string
) {
  dispatch({
    status: NetworkStatus.Connecting,
    network,
    customUrl
  });

  try {
    const connection = new Connection(networkUrl(network, customUrl));
    await connection.getRecentBlockhash();
    dispatch({ status: NetworkStatus.Connected });
  } catch (error) {
    console.error("Failed to update network", error);
    dispatch({ status: NetworkStatus.Failure });
  }
}

export function useNetwork() {
  const context = React.useContext(StateContext);
  if (!context) {
    throw new Error(`useNetwork must be used within a NetworkProvider`);
  }
  return {
    ...context,
    url: networkUrl(context.network, context.customUrl),
    name: networkName(context.network)
  };
}

export function useNetworkDispatch() {
  const context = React.useContext(DispatchContext);
  if (!context) {
    throw new Error(`useNetworkDispatch must be used within a NetworkProvider`);
  }
  return context;
}
