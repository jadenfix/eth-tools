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
fn help_does_not_list_deferred_subcommands() {
    // These belong to follow-up PRs; if they accidentally land in the CLI
    // we want to know immediately.
    let out = cmd().arg("--help").output().expect("run cli");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    for forbidden in [" register ", " invoke ", " watch ", " workers ", " wallet "] {
        assert!(
            !text.contains(forbidden),
            "deferred command {forbidden:?} leaked into --help output:\n{text}"
        );
    }
}

#[test]
fn auth_logout_is_idempotent_without_creds() {
    // No credentials file exists in the fresh tempdir — logout must still
    // succeed (mirrors the unit-test guarantee on `config::clear`).
    cmd().args(["auth", "logout"]).assert().success();
}
