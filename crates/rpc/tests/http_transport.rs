use eth_tools_rpc::{HttpProvider, RotatingProvider, RpcError, RpcProvider};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{mpsc, Arc},
};

fn fixture(status: u16, body: Value) -> (String, mpsc::Receiver<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/inert-key", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let request = loop {
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).unwrap();
            assert_ne!(n, 0);
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                let size: usize = header
                    .lines()
                    .find_map(|s| s.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + size {
                    break serde_json::from_slice(&bytes[end + 4..end + 4 + size]).unwrap();
                }
            }
        };
        tx.send(request).unwrap();
        let body = body.to_string();
        write!(stream,"HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    });
    (url, rx)
}

#[tokio::test]
async fn actual_http_roundtrip_and_rate_limit_failover() {
    let (bad, bad_requests) = fixture(429, json!({}));
    let (good, good_requests) = fixture(200, json!({"jsonrpc":"2.0","id":1,"result":"0x2a"}));
    let rpc = RotatingProvider::new(vec![
        Arc::new(HttpProvider::new("primary", &bad).unwrap()),
        Arc::new(HttpProvider::new("fallback", &good).unwrap()),
    ]);
    assert_eq!(rpc.get_block_number().await.unwrap(), 42);
    for request in [bad_requests.recv().unwrap(), good_requests.recv().unwrap()] {
        assert_eq!(request["method"], "eth_blockNumber");
        assert_eq!(request["params"], json!([]));
    }
}
#[tokio::test]
async fn actual_wire_log_filter_and_response_identity() {
    let (url, requests) = fixture(200, json!({"jsonrpc":"2.0","id":1,"result":[]}));
    let rpc = HttpProvider::new("test", &url).unwrap();
    let filter = alloy::rpc::types::Filter::new().from_block(12).to_block(24);
    assert!(rpc.get_logs(&filter).await.unwrap().is_empty());
    let request = requests.recv().unwrap();
    assert_eq!(request["method"], "eth_getLogs");
    assert_eq!(request["params"][0]["fromBlock"], "0xc");
    assert_eq!(request["params"][0]["toBlock"], "0x18");
    let (url, _requests) = fixture(200, json!({"jsonrpc":"2.0","id":2,"result":"0x2a"}));
    assert!(matches!(
        HttpProvider::new("test", &url).unwrap().get_block_number().await,
        Err(RpcError::Permanent(_))
    ));
}
