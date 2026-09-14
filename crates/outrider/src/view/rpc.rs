//! Loopback TCP JSON-RPC 2.0 server for external tool integration.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

pub type ConnId = u64;

/// Shared atomic flag between RPC/watcher threads and the GPUI pump task.
pub struct Wake {
    pending: AtomicBool,
}

impl Wake {
    pub fn new() -> Self {
        Wake {
            pending: AtomicBool::new(false),
        }
    }
    pub fn raise(&self) {
        self.pending.store(true, Ordering::Release);
    }
    pub fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }
}

/// An inbound JSON-RPC request from a connected client.
pub struct RpcRequest {
    pub id: serde_json::Value,
    pub method: String,
    pub params: serde_json::Value,
    pub reply: mpsc::SyncSender<Outbound>,
}

/// Messages sent back to a connected client.
pub enum Outbound {
    Response(RpcResponse),
    Close,
}

/// A JSON-RPC response (success or error).
pub struct RpcResponse {
    pub id: serde_json::Value,
    pub result: Result<serde_json::Value, RpcError>,
}

/// A JSON-RPC error object.
pub struct RpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct InstanceFile {
    pub pid: u32,
    pub port: u16,
    pub token: String,
    pub project_root: String,
    pub started_at: String,
    pub version: u32,
}

/// TCP JSON-RPC server bound to loopback on a random port.
pub struct RpcServer {
    port: u16,
    #[allow(dead_code)]
    token: String,
    instance_path: PathBuf,
    rx: mpsc::Receiver<RpcRequest>,
    shutdown: Arc<AtomicBool>,
}

const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

fn generate_token() -> String {
    let s1 = RandomState::new();
    let s2 = RandomState::new();
    let mut h1 = s1.build_hasher();
    h1.write_u64(std::process::id() as u64);
    h1.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64,
    );
    let mut h2 = s2.build_hasher();
    h2.write_u64(h1.finish());
    format!("{:016x}{:016x}", h1.finish(), h2.finish())
}

fn instances_dir(cache_root: &Path) -> PathBuf {
    cache_root.join("outrider").join("instances")
}

impl RpcServer {
    /// Start the RPC server using the platform cache directory.
    pub fn start(project_root: &Path, wake: Arc<Wake>) -> Result<RpcServer, String> {
        let cache_root =
            dirs::cache_dir().ok_or_else(|| "no cache directory available".to_string())?;
        Self::start_at(project_root, &cache_root, wake)
    }

    /// Start the RPC server with an explicit cache root (for testing).
    pub fn start_at(
        project_root: &Path,
        cache_root: &Path,
        wake: Arc<Wake>,
    ) -> Result<RpcServer, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("bind failed: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("local_addr: {e}"))?
            .port();
        let token = generate_token();

        // Write instance file.
        let hash = crate::texture_store::project_identity_hash(project_root);
        let inst_dir = instances_dir(cache_root);
        std::fs::create_dir_all(&inst_dir)
            .map_err(|e| format!("create instance dir: {e}"))?;
        let instance_path = inst_dir.join(format!("{hash}.json"));
        let inst = InstanceFile {
            pid: std::process::id(),
            port,
            token: token.clone(),
            project_root: project_root.to_string_lossy().into_owned(),
            started_at: format!(
                "{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ),
            version: 1,
        };
        let json = serde_json::to_string_pretty(&inst)
            .map_err(|e| format!("serialize instance: {e}"))?;
        let tmp_path = instance_path.with_extension("tmp");
        std::fs::write(&tmp_path, &json)
            .map_err(|e| format!("write instance tmp: {e}"))?;
        std::fs::rename(&tmp_path, &instance_path)
            .map_err(|e| format!("rename instance file: {e}"))?;

        let (tx, rx) = mpsc::channel::<RpcRequest>();
        let shutdown = Arc::new(AtomicBool::new(false));

        // Spawn accept thread.
        let accept_shutdown = Arc::clone(&shutdown);
        let accept_token = token.clone();
        let accept_wake = Arc::clone(&wake);
        let accept_port = port;
        std::thread::Builder::new()
            .name("rpc-accept".into())
            .spawn(move || {
                let next_conn = AtomicU64::new(1);
                for stream in listener.incoming() {
                    if accept_shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    let stream = match stream {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let _ = stream.set_nodelay(true);
                    let conn_id = next_conn.fetch_add(1, Ordering::Relaxed);
                    let (out_tx, out_rx) = mpsc::sync_channel::<Outbound>(64);

                    // Writer thread
                    let mut writer_stream = match stream.try_clone() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    std::thread::Builder::new()
                        .name(format!("rpc-write-{conn_id}"))
                        .spawn(move || {
                            for msg in out_rx {
                                match msg {
                                    Outbound::Response(resp) => {
                                        let obj = response_to_json(&resp);
                                        let mut buf =
                                            serde_json::to_vec(&obj).unwrap_or_default();
                                        buf.push(b'\n');
                                        if writer_stream.write_all(&buf).is_err() {
                                            break;
                                        }
                                        let _ = writer_stream.flush();
                                    }
                                    Outbound::Close => break,
                                }
                            }
                        })
                        .ok();

                    // Reader thread
                    let reader_tx = tx.clone();
                    let reader_token = accept_token.clone();
                    let reader_out = out_tx;
                    let reader_wake = Arc::clone(&accept_wake);
                    std::thread::Builder::new()
                        .name(format!("rpc-read-{conn_id}"))
                        .spawn(move || {
                            reader_loop(
                                stream,
                                &reader_token,
                                reader_tx,
                                reader_out,
                                &reader_wake,
                            );
                        })
                        .ok();
                }
                // Accept thread exiting, ignore the unblocking connection.
                let _ = accept_port;
            })
            .map_err(|e| format!("spawn accept thread: {e}"))?;

        Ok(RpcServer {
            port,
            token,
            instance_path,
            rx,
            shutdown,
        })
    }

    /// Drain all pending requests (non-blocking).
    pub fn drain(&self) -> Vec<RpcRequest> {
        self.rx.try_iter().collect()
    }

    /// The TCP port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for RpcServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = std::fs::remove_file(&self.instance_path);
        // Connect to our own port to unblock the accept thread.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// States for the per-connection reader state machine.
enum AuthState {
    AwaitingAuth,
    Authed,
}

fn reader_loop(
    stream: TcpStream,
    expected_token: &str,
    tx: mpsc::Sender<RpcRequest>,
    out: mpsc::SyncSender<Outbound>,
    wake: &Wake,
) {
    let reader = BufReader::new(stream);
    let mut state = AuthState::AwaitingAuth;

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.len() > MAX_LINE_BYTES {
            let _ = out.send(Outbound::Response(RpcResponse {
                id: serde_json::Value::Null,
                result: Err(RpcError {
                    code: -32700,
                    message: "line too long".into(),
                    data: None,
                }),
            }));
            continue;
        }

        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                let _ = out.send(Outbound::Response(RpcResponse {
                    id: serde_json::Value::Null,
                    result: Err(RpcError {
                        code: -32700,
                        message: "parse error".into(),
                        data: None,
                    }),
                }));
                continue;
            }
        };

        if parsed.is_array() || !parsed.is_object() {
            let _ = out.send(Outbound::Response(RpcResponse {
                id: parsed.get("id").cloned().unwrap_or(serde_json::Value::Null),
                result: Err(RpcError {
                    code: -32600,
                    message: "invalid request".into(),
                    data: None,
                }),
            }));
            continue;
        }

        let id = parsed.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = match parsed.get("method").and_then(|m| m.as_str()) {
            Some(m) => m.to_string(),
            None => {
                let _ = out.send(Outbound::Response(RpcResponse {
                    id,
                    result: Err(RpcError {
                        code: -32600,
                        message: "missing method".into(),
                        data: None,
                    }),
                }));
                continue;
            }
        };
        let params = parsed
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        match state {
            AuthState::AwaitingAuth => {
                if method != "auth" {
                    let _ = out.send(Outbound::Response(RpcResponse {
                        id,
                        result: Err(RpcError {
                            code: -32003,
                            message: "auth required".into(),
                            data: None,
                        }),
                    }));
                    let _ = out.send(Outbound::Close);
                    break;
                }
                let token = params
                    .get("token")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                if token != expected_token {
                    let _ = out.send(Outbound::Response(RpcResponse {
                        id,
                        result: Err(RpcError {
                            code: -32003,
                            message: "bad token".into(),
                            data: None,
                        }),
                    }));
                    let _ = out.send(Outbound::Close);
                    break;
                }
                let _ = out.send(Outbound::Response(RpcResponse {
                    id,
                    result: Ok(serde_json::json!({"ok": true})),
                }));
                state = AuthState::Authed;
            }
            AuthState::Authed => {
                let req = RpcRequest {
                    id,
                    method,
                    params,
                    reply: out.clone(),
                };
                if tx.send(req).is_err() {
                    break;
                }
                wake.raise();
            }
        }
    }

    // EOF or error — signal the writer to close.
    let _ = out.send(Outbound::Close);
}

fn response_to_json(resp: &RpcResponse) -> serde_json::Value {
    match &resp.result {
        Ok(result) => serde_json::json!({
            "jsonrpc": "2.0",
            "id": resp.id,
            "result": result,
        }),
        Err(err) => {
            let mut error = serde_json::json!({
                "code": err.code,
                "message": err.message,
            });
            if let Some(data) = &err.data {
                error["data"] = data.clone();
            }
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": resp.id,
                "error": error,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;

    fn read_response(reader: &mut impl BufRead) -> serde_json::Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn instance_file_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let cache = dir.path().join("cache");

        let wake = Arc::new(Wake::new());
        let server = RpcServer::start_at(&project, &cache, Arc::clone(&wake)).unwrap();

        // Instance file should exist.
        let hash = crate::texture_store::project_identity_hash(&project);
        let inst_path = instances_dir(&cache).join(format!("{hash}.json"));
        assert!(inst_path.exists(), "instance file should exist");

        let content = std::fs::read_to_string(&inst_path).unwrap();
        let inst: InstanceFile = serde_json::from_str(&content).unwrap();
        assert_eq!(inst.port, server.port());
        assert_eq!(inst.pid, std::process::id());

        // Drop removes instance file.
        drop(server);
        assert!(
            !inst_path.exists(),
            "instance file should be removed on drop"
        );
    }

    #[test]
    fn auth_wrong_token_closes() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let cache = dir.path().join("cache");

        let wake = Arc::new(Wake::new());
        let server = RpcServer::start_at(&project, &cache, Arc::clone(&wake)).unwrap();
        let port = server.port();

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(b"{\"id\":1,\"method\":\"auth\",\"params\":{\"token\":\"wrong\"}}\n")
            .unwrap();
        stream.flush().unwrap();

        let mut reader = BufReader::new(stream);
        let resp = read_response(&mut reader);
        assert_eq!(resp["error"]["code"], -32003);

        // Give the writer thread time to process the Close message.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Connection should be closed — next read should return EOF or
        // a connection-reset error (platform-dependent).
        let mut next = String::new();
        match reader.read_line(&mut next) {
            Ok(0) => {} // EOF — expected
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {} // Windows
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionAborted => {} // Windows alt
            other => panic!(
                "expected closed connection, got {:?}",
                other
            ),
        }

        drop(server);
    }

    #[test]
    fn auth_correct_token_then_request() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let cache = dir.path().join("cache");

        let wake = Arc::new(Wake::new());
        let server = RpcServer::start_at(&project, &cache, Arc::clone(&wake)).unwrap();
        let port = server.port();

        // Read the instance file to get the token.
        let hash = crate::texture_store::project_identity_hash(&project);
        let inst_path = instances_dir(&cache).join(format!("{hash}.json"));
        let inst: InstanceFile =
            serde_json::from_str(&std::fs::read_to_string(&inst_path).unwrap()).unwrap();

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        // Auth
        let auth_msg = format!(
            "{{\"id\":1,\"method\":\"auth\",\"params\":{{\"token\":\"{}\"}}}}\n",
            inst.token
        );
        stream.write_all(auth_msg.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let resp = read_response(&mut reader);
        assert_eq!(resp["result"]["ok"], true);

        // Send a request
        stream
            .write_all(b"{\"id\":2,\"method\":\"view.get\",\"params\":{}}\n")
            .unwrap();
        stream.flush().unwrap();

        // Wait for the request to arrive and drain it.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let reqs = server.drain();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "view.get");

        // Reply to the request.
        let _ = reqs[0].reply.send(Outbound::Response(RpcResponse {
            id: reqs[0].id.clone(),
            result: Ok(serde_json::json!({"spec": {}})),
        }));

        // Read the response on the client side.
        let resp = read_response(&mut reader);
        assert_eq!(resp["id"], 2);
        assert!(resp["result"]["spec"].is_object());

        drop(server);
    }

    #[test]
    fn wake_flag_raised_on_request() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let cache = dir.path().join("cache");

        let wake = Arc::new(Wake::new());
        let server = RpcServer::start_at(&project, &cache, Arc::clone(&wake)).unwrap();
        let port = server.port();

        // Read instance for token.
        let hash = crate::texture_store::project_identity_hash(&project);
        let inst_path = instances_dir(&cache).join(format!("{hash}.json"));
        let inst: InstanceFile =
            serde_json::from_str(&std::fs::read_to_string(&inst_path).unwrap()).unwrap();

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let auth_msg = format!(
            "{{\"id\":1,\"method\":\"auth\",\"params\":{{\"token\":\"{}\"}}}}\n",
            inst.token
        );
        stream.write_all(auth_msg.as_bytes()).unwrap();
        stream.flush().unwrap();

        // Consume auth response.
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let _ = read_response(&mut reader);

        // Clear any existing wake.
        wake.take();

        // Send a request.
        stream
            .write_all(b"{\"id\":2,\"method\":\"test.ping\",\"params\":{}}\n")
            .unwrap();
        stream.flush().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(wake.take(), "wake flag should be raised after request");

        drop(server);
    }
}
