use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

pub enum RpcFailure {
    Io(String),
    Violation { violations: Vec<Value> },
    Rpc { code: i32, message: String, #[allow(dead_code)] data: Option<Value> },
}

impl std::fmt::Display for RpcFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            RpcFailure::Io(e) => write!(f, "connection error: {e}"),
            RpcFailure::Violation { violations } => {
                for v in violations {
                    let path = v.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                    let msg = v.get("message").and_then(|v| v.as_str()).unwrap_or("?");
                    let rule = v.get("rule").and_then(|v| v.as_str()).unwrap_or("?");
                    writeln!(f, "error: {path} -- {msg} ({rule})")?;
                }
                Ok(())
            }
            RpcFailure::Rpc { code, message, .. } => write!(f, "RPC error {code}: {message}"),
        }
    }
}

impl Client {
    pub fn connect(port: u16, token: &str, timeout: Duration) -> Result<Client, RpcFailure> {
        let stream = TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            timeout,
        )
        .map_err(|e| {
            RpcFailure::Io(format!(
                "cannot connect to outrider on port {port}: {e}"
            ))
        })?;
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(timeout)).ok();

        let writer = stream
            .try_clone()
            .map_err(|e| RpcFailure::Io(e.to_string()))?;
        let reader = BufReader::new(stream);
        let mut client = Client {
            reader,
            writer,
            next_id: 1,
        };

        // Auth handshake
        let auth_result = client.call_raw(0, "auth", json!({"token": token}))?;
        if auth_result.get("error").is_some() {
            return Err(RpcFailure::Io(
                "authentication failed -- token rejected".into(),
            ));
        }

        Ok(client)
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcFailure> {
        let id = self.next_id;
        self.next_id += 1;
        self.call_raw(id, method, params)
    }

    fn call_raw(&mut self, id: u64, method: &str, params: Value) -> Result<Value, RpcFailure> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line =
            serde_json::to_string(&request).map_err(|e| RpcFailure::Io(e.to_string()))?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .map_err(|e| RpcFailure::Io(e.to_string()))?;
        self.writer
            .flush()
            .map_err(|e| RpcFailure::Io(e.to_string()))?;

        // Read response (skip notifications)
        loop {
            let mut buf = String::new();
            let n = self
                .reader
                .read_line(&mut buf)
                .map_err(|e| RpcFailure::Io(e.to_string()))?;
            if n == 0 {
                return Err(RpcFailure::Io("server closed connection".into()));
            }
            let response: Value = serde_json::from_str(buf.trim())
                .map_err(|e| RpcFailure::Io(format!("invalid response: {e}")))?;

            // Skip notifications (no "id" field)
            if response.get("id").is_none() {
                continue;
            }

            // Check if id matches
            if response["id"].as_u64() != Some(id) {
                continue;
            }

            if let Some(error) = response.get("error") {
                let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0) as i32;
                let message = error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let data = error.get("data").cloned();
                if code == -32001 {
                    let violations = data
                        .as_ref()
                        .and_then(|d| d.as_array())
                        .cloned()
                        .unwrap_or_default();
                    return Err(RpcFailure::Violation { violations });
                }
                return Err(RpcFailure::Rpc {
                    code,
                    message,
                    data,
                });
            }

            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}
