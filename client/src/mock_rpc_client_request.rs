use crate::generic_rpc_client_request::GenericRpcClientRequest;
use crate::rpc_request::RpcRequest;
use serde_json::{Number, Value};

pub const PUBKEY: &str = "7RoSF9fUmdphVCpabEoefH81WwrW7orsWonXWqTXkKV8";
pub const SIGNATURE: &str =
    "43yNSFC6fYTuPgTNFFhF4axw7AfWxB2BPdurme8yrsWEYwm8299xh8n6TAHjGymiSub1XtyxTNyd9GBfY2hxoBw8";

pub struct MockRpcClientRequest {
    url: String,
}

impl MockRpcClientRequest {
    pub fn new(url: String) -> Self {
        Self { url }
    }
}

impl GenericRpcClientRequest for MockRpcClientRequest {
    fn send(
        &self,
        request: &RpcRequest,
        params: Option<serde_json::Value>,
        _retries: usize,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        if self.url == "fails" {
            return Ok(Value::Null);
        }
        let val = match request {
            RpcRequest::ConfirmTransaction => {
                if let Some(Value::Array(param_array)) = params {
                    if let Value::String(param_string) = &param_array[0] {
                        Value::Bool(param_string == SIGNATURE)
                    } else {
                        Value::Null
                    }
                } else {
                    Value::Null
                }
            }
            RpcRequest::GetBalance => {
                let n = if self.url == "airdrop" { 0 } else { 50 };
                Value::Number(Number::from(n))
            }
            RpcRequest::GetRecentBlockhash => Value::String(PUBKEY.to_string()),
            RpcRequest::GetSignatureStatus => {
                let str = if self.url == "account_in_use" {
                    "AccountInUse"
                } else if self.url == "bad_sig_status" {
                    "SignatureNotFound"
                } else {
                    "Confirmed"
                };
                Value::String(str.to_string())
            }
            RpcRequest::GetTransactionCount => Value::Number(Number::from(1234)),
            RpcRequest::SendTransaction => Value::String(SIGNATURE.to_string()),
            _ => Value::Null,
        };
        Ok(val)
    }
}
