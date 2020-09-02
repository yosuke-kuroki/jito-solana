use log::*;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{
    json,
    value::Value::{Number, Object},
    Map, Value,
};
use solana_sdk::{
    commitment_config::CommitmentConfig, signature::Signature, transaction::TransactionError,
};
use std::{
    marker::PhantomData,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver},
        Arc, RwLock,
    },
    thread::JoinHandle,
};
use thiserror::Error;
use tungstenite::{client::AutoStream, connect, Message, WebSocket};
use url::{ParseError, Url};

type PubsubSignatureResponse = PubsubClientSubscription<RpcResponse<SignatureResult>>;

#[derive(Debug, Error)]
pub enum PubsubClientError {
    #[error("url parse error")]
    UrlParseError(#[from] ParseError),

    #[error("unable to connect to server")]
    ConnectionError(#[from] tungstenite::Error),

    #[error("json parse error")]
    JsonParseError(#[from] serde_json::error::Error),

    #[error("unexpected message format")]
    UnexpectedMessageError,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type", content = "result")]
pub enum SignatureResult {
    ProcessedSignatureResult(ProcessedSignatureResult),
    ReceivedSignature,
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
pub struct RpcResponseContext {
    pub slot: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessedSignatureResult {
    pub err: Option<TransactionError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse<T> {
    pub context: RpcResponseContext,
    pub value: T,
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
pub struct SlotInfoMessage {
    pub parent: u64,
    pub root: u64,
    pub slot: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSignatureSubscribeConfig {
    #[serde(flatten)]
    pub commitment: Option<CommitmentConfig>,
    pub enable_received_notification: Option<bool>,
}

pub struct PubsubClientSubscription<T>
where
    T: DeserializeOwned,
{
    message_type: PhantomData<T>,
    operation: &'static str,
    socket: Arc<RwLock<WebSocket<AutoStream>>>,
    subscription_id: u64,
    t_cleanup: Option<JoinHandle<()>>,
    exit: Arc<AtomicBool>,
}

impl<T> Drop for PubsubClientSubscription<T>
where
    T: DeserializeOwned,
{
    fn drop(&mut self) {
        self.send_unsubscribe()
            .unwrap_or_else(|_| warn!("unable to unsubscribe from websocket"));
        self.socket
            .write()
            .unwrap()
            .close(None)
            .unwrap_or_else(|_| warn!("unable to close websocket"));
    }
}

impl<T> PubsubClientSubscription<T>
where
    T: DeserializeOwned,
{
    fn send_subscribe(
        writable_socket: &Arc<RwLock<WebSocket<AutoStream>>>,
        body: String,
    ) -> Result<u64, PubsubClientError> {
        writable_socket
            .write()
            .unwrap()
            .write_message(Message::Text(body))?;
        let message = writable_socket.write().unwrap().read_message()?;
        Self::extract_subscription_id(message)
    }

    fn extract_subscription_id(message: Message) -> Result<u64, PubsubClientError> {
        let message_text = &message.into_text()?;
        let json_msg: Map<String, Value> = serde_json::from_str(message_text)?;

        if let Some(Number(x)) = json_msg.get("result") {
            if let Some(x) = x.as_u64() {
                return Ok(x);
            }
        }

        Err(PubsubClientError::UnexpectedMessageError)
    }

    pub fn send_unsubscribe(&self) -> Result<(), PubsubClientError> {
        let method = format!("{}Unubscribe", self.operation);
        self.socket
            .write()
            .unwrap()
            .write_message(Message::Text(
                json!({
                "jsonrpc":"2.0","id":1,"method":method,"params":[self.subscription_id]
                })
                .to_string(),
            ))
            .map_err(|err| err.into())
    }

    fn read_message(
        writable_socket: &Arc<RwLock<WebSocket<AutoStream>>>,
    ) -> Result<T, PubsubClientError> {
        let message = writable_socket.write().unwrap().read_message()?;
        let message_text = &message.into_text().unwrap();
        let json_msg: Map<String, Value> = serde_json::from_str(message_text)?;

        if let Some(Object(value_1)) = json_msg.get("params") {
            if let Some(value_2) = value_1.get("result") {
                let x: T = serde_json::from_value::<T>(value_2.clone()).unwrap();
                return Ok(x);
            }
        }

        Err(PubsubClientError::UnexpectedMessageError)
    }

    pub fn shutdown(&mut self) -> std::thread::Result<()> {
        if self.t_cleanup.is_some() {
            info!("websocket thread - shutting down");
            self.exit.store(true, Ordering::Relaxed);
            let x = self.t_cleanup.take().unwrap().join();
            info!("websocket thread - shut down.");
            x
        } else {
            warn!("websocket thread - already shut down.");
            Ok(())
        }
    }
}

const SLOT_OPERATION: &str = "slot";
const SIGNATURE_OPERATION: &str = "signature";

pub struct PubsubClient {}

impl PubsubClient {
    pub fn slot_subscribe(
        url: &str,
    ) -> Result<
        (
            PubsubClientSubscription<SlotInfoMessage>,
            Receiver<SlotInfoMessage>,
        ),
        PubsubClientError,
    > {
        let url = Url::parse(url)?;
        let (socket, _response) = connect(url)?;
        let (sender, receiver) = channel::<SlotInfoMessage>();

        let socket = Arc::new(RwLock::new(socket));
        let socket_clone = socket.clone();
        let exit = Arc::new(AtomicBool::new(false));
        let exit_clone = exit.clone();
        let subscription_id = PubsubClientSubscription::<SlotInfoMessage>::send_subscribe(
            &socket_clone,
            json!({
                "jsonrpc":"2.0","id":1,"method":format!("{}Subscribe", SLOT_OPERATION),"params":[]
            })
            .to_string(),
        )
        .unwrap();

        let t_cleanup = std::thread::spawn(move || {
            loop {
                if exit_clone.load(Ordering::Relaxed) {
                    break;
                }

                let message: Result<SlotInfoMessage, PubsubClientError> =
                    PubsubClientSubscription::read_message(&socket_clone);

                if let Ok(msg) = message {
                    match sender.send(msg.clone()) {
                        Ok(_) => (),
                        Err(err) => {
                            info!("receive error: {:?}", err);
                            break;
                        }
                    }
                } else {
                    info!("receive error: {:?}", message);
                    break;
                }
            }

            info!("websocket - exited receive loop");
        });

        let result: PubsubClientSubscription<SlotInfoMessage> = PubsubClientSubscription {
            message_type: PhantomData,
            operation: SLOT_OPERATION,
            socket,
            subscription_id,
            t_cleanup: Some(t_cleanup),
            exit,
        };

        Ok((result, receiver))
    }

    pub fn signature_subscribe(
        url: &str,
        signature: &Signature,
    ) -> Result<
        (
            PubsubSignatureResponse,
            Receiver<RpcResponse<SignatureResult>>,
        ),
        PubsubClientError,
    > {
        let url = Url::parse(url)?;
        let (socket, _response) = connect(url)?;
        let (sender, receiver) = channel::<RpcResponse<SignatureResult>>();

        let socket = Arc::new(RwLock::new(socket));
        let socket_clone = socket.clone();
        let exit = Arc::new(AtomicBool::new(false));
        let exit_clone = exit.clone();
        let body = json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":format!("{}Subscribe", SIGNATURE_OPERATION),
            "params":[
                signature.to_string(),
                {"enableReceivedNotification": true }
            ]
        })
        .to_string();
        let subscription_id =
            PubsubClientSubscription::<RpcResponse<SignatureResult>>::send_subscribe(
                &socket_clone,
                body,
            )
            .unwrap();

        let t_cleanup = std::thread::spawn(move || {
            loop {
                if exit_clone.load(Ordering::Relaxed) {
                    break;
                }

                let message: Result<RpcResponse<SignatureResult>, PubsubClientError> =
                    PubsubClientSubscription::read_message(&socket_clone);

                if let Ok(msg) = message {
                    match sender.send(msg.clone()) {
                        Ok(_) => (),
                        Err(err) => {
                            info!("receive error: {:?}", err);
                            break;
                        }
                    }
                } else {
                    info!("receive error: {:?}", message);
                    break;
                }
            }

            info!("websocket - exited receive loop");
        });

        let result: PubsubClientSubscription<RpcResponse<SignatureResult>> =
            PubsubClientSubscription {
                message_type: PhantomData,
                operation: SIGNATURE_OPERATION,
                socket,
                subscription_id,
                t_cleanup: Some(t_cleanup),
                exit,
            };

        Ok((result, receiver))
    }
}

#[cfg(test)]
mod tests {
    // see core/tests/client.rs#test_slot_subscription()
}
