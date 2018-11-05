use reqwest;
use reqwest::header::CONTENT_TYPE;
use serde_json::{self, Value};
use std::{error, fmt};

pub enum RpcRequest {
    ConfirmTransaction,
    GetAccountInfo,
    GetBalance,
    GetFinality,
    GetLastId,
    GetSignatureStatus,
    GetTransactionCount,
    RequestAirdrop,
    SendTransaction,
}
impl RpcRequest {
    pub fn make_rpc_request(
        &self,
        rpc_addr: &str,
        id: u64,
        params: Option<Value>,
    ) -> Result<Value, Box<error::Error>> {
        let request = self.build_request_json(id, params);
        let client = reqwest::Client::new();
        let mut response = client
            .post(rpc_addr)
            .header(CONTENT_TYPE, "application/json")
            .body(request.to_string())
            .send()?;
        let json: Value = serde_json::from_str(&response.text()?)?;
        if json["error"].is_object() {
            Err(RpcError::RpcRequestError(format!(
                "RPC Error response: {}",
                serde_json::to_string(&json["error"]).unwrap()
            )))?
        }
        Ok(json["result"].clone())
    }
    fn build_request_json(&self, id: u64, params: Option<Value>) -> Value {
        let jsonrpc = "2.0";
        let method = match self {
            RpcRequest::ConfirmTransaction => "confirmTransaction",
            RpcRequest::GetAccountInfo => "getAccountInfo",
            RpcRequest::GetBalance => "getBalance",
            RpcRequest::GetFinality => "getFinality",
            RpcRequest::GetLastId => "getLastId",
            RpcRequest::GetSignatureStatus => "getSignatureStatus",
            RpcRequest::GetTransactionCount => "getTransactionCount",
            RpcRequest::RequestAirdrop => "requestAirdrop",
            RpcRequest::SendTransaction => "sendTransaction",
        };
        let mut request = json!({
           "jsonrpc": jsonrpc,
           "id": id,
           "method": method,
        });
        if let Some(param_string) = params {
            request["params"] = json!(vec![param_string]);
        }
        request
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RpcError {
    RpcRequestError(String),
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "invalid")
    }
}

impl error::Error for RpcError {
    fn description(&self) -> &str {
        "invalid"
    }

    fn cause(&self) -> Option<&error::Error> {
        // Generic error, underlying cause isn't tracked.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpc_core::*;
    use jsonrpc_http_server::*;
    use serde_json::Number;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::mpsc::channel;
    use std::thread;

    #[test]
    fn test_build_request_json() {
        let test_request = RpcRequest::GetAccountInfo;
        let request = test_request.build_request_json(
            1,
            Some(json!("deadbeefXjn8o3yroDHxUtKsZZgoy4GPkPPXfouKNHhx")),
        );
        assert_eq!(request["method"], "getAccountInfo");
        assert_eq!(
            request["params"],
            json!(vec!["deadbeefXjn8o3yroDHxUtKsZZgoy4GPkPPXfouKNHhx"])
        );

        let test_request = RpcRequest::GetBalance;
        let request = test_request.build_request_json(
            1,
            Some(json!("deadbeefXjn8o3yroDHxUtKsZZgoy4GPkPPXfouKNHhx")),
        );
        assert_eq!(request["method"], "getBalance");

        let test_request = RpcRequest::GetFinality;
        let request = test_request.build_request_json(1, None);
        assert_eq!(request["method"], "getFinality");
        assert_eq!(request["params"], json!(null));

        let test_request = RpcRequest::GetLastId;
        let request = test_request.build_request_json(1, None);
        assert_eq!(request["method"], "getLastId");

        let test_request = RpcRequest::GetTransactionCount;
        let request = test_request.build_request_json(1, None);
        assert_eq!(request["method"], "getTransactionCount");

        let test_request = RpcRequest::RequestAirdrop;
        let request = test_request.build_request_json(1, None);
        assert_eq!(request["method"], "requestAirdrop");

        let test_request = RpcRequest::SendTransaction;
        let request = test_request.build_request_json(1, None);
        assert_eq!(request["method"], "sendTransaction");
    }
    #[test]
    fn test_make_rpc_request() {
        let (sender, receiver) = channel();
        thread::spawn(move || {
            let rpc_addr = socketaddr!(0, 0);
            let mut io = IoHandler::default();
            // Successful request
            io.add_method("getBalance", |_params: Params| {
                Ok(Value::Number(Number::from(50)))
            });
            // Failed request
            io.add_method("getLastId", |params: Params| {
                if params != Params::None {
                    Err(Error::invalid_request())
                } else {
                    Ok(Value::String(
                        "deadbeefXjn8o3yroDHxUtKsZZgoy4GPkPPXfouKNHhx".to_string(),
                    ))
                }
            });

            let server = ServerBuilder::new(io)
                .threads(1)
                .cors(DomainsValidation::AllowOnly(vec![
                    AccessControlAllowOrigin::Any,
                ])).start_http(&rpc_addr)
                .expect("Unable to start RPC server");
            sender.send(*server.address()).unwrap();
            server.wait();
        });

        let rpc_addr = receiver.recv().unwrap();
        let rpc_addr = format!("http://{}", rpc_addr.to_string());

        let balance = RpcRequest::GetBalance.make_rpc_request(
            &rpc_addr,
            1,
            Some(json!("deadbeefXjn8o3yroDHxUtKsZZgoy4GPkPPXfouKNHhx")),
        );
        assert!(balance.is_ok());
        assert_eq!(balance.unwrap().as_u64().unwrap(), 50);

        let last_id = RpcRequest::GetLastId.make_rpc_request(&rpc_addr, 2, None);
        assert!(last_id.is_ok());
        assert_eq!(
            last_id.unwrap().as_str().unwrap(),
            "deadbeefXjn8o3yroDHxUtKsZZgoy4GPkPPXfouKNHhx"
        );

        // Send erroneous parameter
        let last_id = RpcRequest::GetLastId.make_rpc_request(&rpc_addr, 3, Some(json!("paramter")));
        assert_eq!(last_id.is_err(), true);
    }
}
