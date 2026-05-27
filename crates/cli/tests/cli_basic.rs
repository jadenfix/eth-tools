//! Smoke tests for the top-level binary surface — `--version`, `--help`, and
//! the `logout` no-network path. These are guard rails for trivial regressions
//! in clap wiring or the help text.

use assert_cmd::Command;
use predicates::prelude::*;

fn cmd() -> Command {
    let mut c = Command::cargo_bin("eth-tools").expect("binary built");
    // Isolate from the developer's real ~/.config dir.
    let tmp = tempfile::tempdir().expect("tempdir");
    c.env("ETH_TOOLS_CONFIG_DIR", tmp.path());
    // Leak the tempdir intentionally: it's cleaned up at process exit.
    std::mem::forget(tmp);
    c
}

#[test]
fn version_flag_prints_crate_version() {
    cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_lists_in_scope_subcommands() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("auth"))
        .stdout(predicate::str::contains("find"))
        .stdout(predicate::str::contains("inspect"))
        .stdout(predicate::str::contains("manifest"))
        .stdout(predicate::str::contains("health"));
}

#[test]
fn help_lists_phase_7_completion_subcommands() {
    // Inverted (post phase-7 completion) from the earlier "deferred" guard:
    // all 13 plan §9.3 commands must now show in `--help`. If one drops out
    // (e.g. a clap derive bug or a removed enum variant), this test fails.
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("register"))
        .stdout(predicate::str::contains("invoke"))
        .stdout(predicate::str::contains("watch"))
        .stdout(predicate::str::contains("mcp"))
        .stdout(predicate::str::contains("workers"))
        .stdout(predicate::str::contains("wallet"))
        .stdout(predicate::str::contains("backfill"));
}

#[test]
fn mcp_subcommands_listed() {
    cmd()
        .args(["mcp", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("install"))
        .stdout(predicate::str::contains("from-card"));
}

#[test]
fn workers_subcommands_listed() {
    cmd()
        .args(["workers", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("status"));
}

#[test]
fn wallet_subcommands_listed() {
    cmd()
        .args(["wallet", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("status"));
}

#[test]
fn register_requires_a_mode() {
    // Neither --interactive nor --manifest => clear error, no network.
    cmd()
        .arg("register")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--interactive").or(predicate::str::contains("--manifest")));
}

#[test]
fn auth_logout_is_idempotent_without_creds() {
    // No credentials file exists in the fresh tempdir — logout must still
    // succeed (mirrors the unit-test guarantee on `config::clear`).
    cmd().args(["auth", "logout"]).assert().success();
}
