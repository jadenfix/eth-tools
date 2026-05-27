//! HTTP-driven integration tests for read commands. Every test stands up a
//! `wiremock` server on localhost so we never touch `https://eth-tools.dev`.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a CLI command pointed at `base_url` with an isolated config dir.
fn cli(base_url: &str) -> Command {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut c = Command::cargo_bin("eth-tools").expect("binary built");
    c.env("ETH_TOOLS_CONFIG_DIR", tmp.path())
        .env("ETH_TOOLS_API_URL", base_url);
    std::mem::forget(tmp);
    c
}

#[tokio::test]
async fn health_renders_status_and_chain_table() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "ok",
            "version": "0.0.1",
            "agents_indexed": 7,
            "chains": [
                { "chain_id": 8453, "name": "base", "is_testnet": false, "agents_indexed": 5 },
                { "chain_id": 84532, "name": "base-sepolia", "is_testnet": true, "agents_indexed": 2 }
            ]
        })))
        .mount(&server)
        .await;

    cli(&server.uri())
        .arg("health")
        .assert()
        .success()
        .stdout(predicate::str::contains("status:  ok"))
        .stdout(predicate::str::contains("base"))
        .stdout(predicate::str::contains("8453"));
}

#[tokio::test]
async fn health_json_flag_emits_raw_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "ok",
            "version": "0.0.1",
            "agents_indexed": 0,
            "chains": []
        })))
        .mount(&server)
        .await;

    cli(&server.uri())
        .args(["--json", "health"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"ok\""));
}

#[tokio::test]
async fn find_renders_table_from_search_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/agents/search"))
        .and(body_json(json!({ "query": "risk" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {
                    "chain": "base",
                    "chain_id": 8453,
                    "agent_id": "42",
                    "owner": "0xdead",
                    "agent_uri": "https://example.com/agent.json",
                    "registered_at": "2024-01-01T00:00:00Z",
                    "updated_at": "2024-01-02T00:00:00Z"
                }
            ]
        })))
        .mount(&server)
        .await;

    cli(&server.uri())
        .args(["find", "risk"])
        .assert()
        .success()
        .stdout(predicate::str::contains("base"))
        .stdout(predicate::str::contains("42"))
        .stdout(predicate::str::contains("0xdead"));
}

#[tokio::test]
async fn inspect_renders_agent_detail() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/agents/base/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "chain": "base",
                "chain_id": 8453,
                "agent_id": "42",
                "owner": "0xdead",
                "agent_uri": "https://example.com/agent.json",
                "agent_wallet": null,
                "registered_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-02T00:00:00Z"
            },
            "source": "db",
            "staleness_ms": 0
        })))
        .mount(&server)
        .await;

    cli(&server.uri())
        .args(["inspect", "base/42"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agent_id"))
        .stdout(predicate::str::contains("42"))
        .stdout(predicate::str::contains("source: db"));
}

#[tokio::test]
async fn inspect_rejects_malformed_reference() {
    cli("http://127.0.0.1:1") // unreachable, but we should never get there
        .args(["inspect", "no-slash"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("expected <chain>/<agent_id>"));
}

#[tokio::test]
async fn manifest_validate_against_good_fixture() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/manifest/validate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "valid": true,
            "errors": []
        })))
        .mount(&server)
        .await;

    // Resolve the workspace fixture relative to CARGO_MANIFEST_DIR (the
    // crate dir), not the workspace root, so the test is portable.
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/agent-card.good.json")
        .canonicalize()
        .expect("fixture present at workspace root");

    cli(&server.uri())
        .args(["manifest", "validate"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("manifest: ok"));
}

#[tokio::test]
async fn manifest_validate_shows_pointer_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/manifest/validate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "valid": false,
            "errors": [
                { "pointer": "/services/0/endpoint", "message": "must be https" }
            ]
        })))
        .mount(&server)
        .await;

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/agent-card.bad.json")
        .canonicalize()
        .expect("bad fixture present");

    cli(&server.uri())
        .args(["manifest", "validate"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("manifest: invalid"))
        .stdout(predicate::str::contains("/services/0/endpoint"))
        .stdout(predicate::str::contains("must be https"));
}

#[tokio::test]
async fn manifest_hash_prints_both_digests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/manifest/hash"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha256":    "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "keccak256": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        })))
        .mount(&server)
        .await;

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/agent-card.good.json")
        .canonicalize()
        .expect("fixture present");

    cli(&server.uri())
        .args(["manifest", "hash"])
        .arg(&fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("sha256:"))
        .stdout(predicate::str::contains("0xaaaa"))
        .stdout(predicate::str::contains("keccak256:"))
        .stdout(predicate::str::contains("0xbbbb"));
}

#[tokio::test]
async fn request_times_out_when_server_stalls() {
    // B1 guard: prove the 30s total timeout actually fires. Without
    // `.timeout(...)` on the reqwest builder, this test would hang for the
    // wiremock delay (60s) until the harness killed it.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
        .mount(&server)
        .await;

    let start = std::time::Instant::now();
    let out = cli(&server.uri())
        .arg("health")
        .timeout(Duration::from_secs(45))
        .output()
        .expect("run cli");
    let elapsed = start.elapsed();
    assert!(
        !out.status.success(),
        "health was supposed to time out, got success"
    );
    assert!(
        elapsed < Duration::from_secs(40),
        "client timeout should fire within ~30s, observed {elapsed:?}"
    );
}

#[test]
fn rejects_http_remote_api_url_at_parse_time() {
    // Clap's `value_parser` runs against `--api-url`, the env var, and the
    // default; passing a non-localhost http:// URL must fail BEFORE any
    // network I/O so a bearer token can never leave the process in cleartext.
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut c = Command::cargo_bin("eth-tools").expect("binary built");
    c.env("ETH_TOOLS_CONFIG_DIR", tmp.path())
        // Don't let the developer's env spill `ETH_TOOLS_API_URL`.
        .env_remove("ETH_TOOLS_API_URL");
    std::mem::forget(tmp);
    c.args(["--api-url", "http://example.com", "health"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("plaintext http://"));
}

#[test]
fn accepts_http_localhost_url() {
    // 127.0.0.1 is on the allow-list (this is what `cli_http.rs` itself relies
    // on for the wiremock tests).
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut c = Command::cargo_bin("eth-tools").expect("binary built");
    c.env("ETH_TOOLS_CONFIG_DIR", tmp.path());
    std::mem::forget(tmp);
    // We don't bring up a server here — just confirm clap accepted the URL.
    // The network call will fail (port closed), but that failure proves we
    // got past the parse-time check.
    let out = c
        .args(["--api-url", "http://127.0.0.1:1", "health"])
        .output()
        .expect("run cli");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("plaintext http://"),
        "http://127.0.0.1 must be accepted; got: {stderr}"
    );
}

#[tokio::test]
async fn register_interactive_drives_full_flow_via_stdin() {
    // Server stubs the two endpoints the interactive flow hits:
    //   POST /api/v1/manifest/generate -> assembled manifest
    //   POST /api/v1/manifest/hash     -> sha256/keccak256
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/manifest/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "My Agent",
            "skills": ["risk"],
            "services": []
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/manifest/hash"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha256": "0xaa",
            "keccak256": "0xbb"
        })))
        .mount(&server)
        .await;

    // Scripted stdin: name, description, skills, "n" to no extra services.
    // RealRegisterIo reads via std::io::stdin().read_line which assert_cmd's
    // .write_stdin can drive directly.
    let script = "My Agent\nA test agent\nrisk\nn\n";

    cli(&server.uri())
        .arg("register")
        .arg("--interactive")
        .write_stdin(script)
        .assert()
        .success()
        .stdout(predicate::str::contains("manifest assembled"))
        .stdout(predicate::str::contains("0xaa"))
        .stdout(predicate::str::contains("0xbb"));
}

#[tokio::test]
async fn invoke_renders_output_field() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/invoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": { "score": 42 },
            "request_id": "req_abc"
        })))
        .mount(&server)
        .await;

    // Write a tiny input file the CLI can read.
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let input_path = tmpdir.path().join("input.json");
    std::fs::write(&input_path, br#"{"address": "0xdead"}"#).unwrap();

    cli(&server.uri())
        .args(["invoke", "base/42", "--input"])
        .arg(&input_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("output:"))
        .stdout(predicate::str::contains("request_id: req_abc"));

    // Leak tempdir so it survives the assert.
    std::mem::forget(tmpdir);
}

#[tokio::test]
async fn invoke_surfaces_402_payment_required_cleanly() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/invoke"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "error": { "code": "PAYMENT_REQUIRED", "evaluator": "x402" }
        })))
        .mount(&server)
        .await;

    let tmpdir = tempfile::tempdir().expect("tempdir");
    let input_path = tmpdir.path().join("input.json");
    std::fs::write(&input_path, b"{}").unwrap();

    cli(&server.uri())
        .args(["invoke", "base/42", "--input"])
        .arg(&input_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("payment required"))
        .stderr(predicate::str::contains("--x-payment"));
    std::mem::forget(tmpdir);
}

#[tokio::test]
async fn watch_polls_once_with_max_iters_and_prints_rows() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/agents"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {
                    "chain": "base",
                    "agent_id": "1",
                    "owner": "0xfeed",
                    "agent_uri": "https://example.com/a.json"
                }
            ]
        })))
        .mount(&server)
        .await;

    cli(&server.uri())
        .args(["watch", "base", "--max-iters", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[base] 1"))
        .stdout(predicate::str::contains("0xfeed"));
}

#[tokio::test]
async fn workers_status_renders_table() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/workers/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "workers": [
                {
                    "name": "registry_scraper",
                    "last_ok_at": "2024-01-01T00:00:00Z",
                    "age_seconds": 42,
                    "cursor_lag": 3,
                    "env": "production"
                }
            ]
        })))
        .mount(&server)
        .await;

    cli(&server.uri())
        .args(["workers", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("registry_scraper"))
        .stdout(predicate::str::contains("production"));
}

#[tokio::test]
async fn workers_status_404_prints_not_yet_deployed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/workers/status"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": { "code": "NOT_FOUND" }
        })))
        .mount(&server)
        .await;

    cli(&server.uri())
        .args(["workers", "status"])
        .assert()
        .success() // Soft-fail: not yet deployed should NOT crash the CLI.
        .stderr(predicate::str::contains("not yet deployed"));
}

#[tokio::test]
async fn wallet_status_renders_balance_and_kill_switch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/wallet/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "balance_usdc": "12.34",
            "spend_today_usdc": "0.50",
            "kill_switch": false,
            "allowlist": ["0xabc", "0xdef"]
        })))
        .mount(&server)
        .await;

    cli(&server.uri())
        .args(["wallet", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("balance USDC:"))
        .stdout(predicate::str::contains("12.34"))
        .stdout(predicate::str::contains("kill switch:      off"))
        .stdout(predicate::str::contains("0xabc"));
}

#[tokio::test]
async fn wallet_status_404_prints_not_yet_deployed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/wallet/status"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    cli(&server.uri())
        .args(["wallet", "status"])
        .assert()
        .success()
        .stderr(predicate::str::contains("not yet deployed"));
}

#[tokio::test]
async fn mcp_from_card_emits_ts_with_card_data() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/agent-card.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "Test Scorer",
            "services": [
                { "type": "A2A", "endpoint": "https://example.com/a2a" }
            ]
        })))
        .mount(&server)
        .await;

    let tmpdir = tempfile::tempdir().expect("tempdir");
    let out_path = tmpdir.path().join("out.ts");
    let url = format!("{}/agent-card.json", server.uri());

    cli(&server.uri())
        .args(["mcp", "from-card"])
        .arg(&url)
        .args(["--out"])
        .arg(&out_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"));

    let ts = std::fs::read_to_string(&out_path).unwrap();
    assert!(ts.contains("Test Scorer"), "TS should embed name: {ts}");
    assert!(ts.contains("https://example.com/a2a"), "TS should embed endpoint");
    std::mem::forget(tmpdir);
}

#[tokio::test]
async fn backfill_dry_run_uses_rpc_only() {
    // Stand up a mock RPC server that answers eth_blockNumber.
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x10" // 16
        })))
        .mount(&rpc)
        .await;

    // The API server should NOT be hit at all — backfill only talks to RPC.
    let api = MockServer::start().await;

    cli(&api.uri())
        .env("RPC_URL_PRIMARY", rpc.uri())
        .args(["backfill", "--chain", "base", "--from-block", "5", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("from-block:  5"))
        .stdout(predicate::str::contains("to-block:    16"))
        .stdout(predicate::str::contains("span:        11"))
        .stdout(predicate::str::contains("(dry-run)"));
}

#[tokio::test]
async fn backfill_non_dry_run_prints_not_yet_exposed() {
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x100"
        })))
        .mount(&rpc)
        .await;
    let api = MockServer::start().await;

    cli(&api.uri())
        .env("RPC_URL_PRIMARY", rpc.uri())
        .args(["backfill", "--chain", "base", "--from-block", "0"])
        .assert()
        .success()
        .stderr(predicate::str::contains("not yet exposed"));
}

#[tokio::test]
async fn api_error_body_is_surfaced_verbatim() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/agents/base/9999"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {
                "code": "AGENT_NOT_FOUND",
                "evaluator": "api.lookup",
                "policy_version": "v1"
            }
        })))
        .mount(&server)
        .await;

    cli(&server.uri())
        .args(["inspect", "base/9999"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("AGENT_NOT_FOUND"));
}
