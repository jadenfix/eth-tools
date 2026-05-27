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
