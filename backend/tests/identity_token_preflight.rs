use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

const MALFORMED_TOKEN: &str = "sensitive.invalid-token";
const ARG_TOKEN: &str = "sensitive-argv-token";

fn jwt_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_verify_identity_access_token"));
    command
        .env_clear()
        .env("AUTH_MODE", "jwt")
        .env("AUTH_ISSUER", "https://id.example.test")
        .env("AUTH_AUDIENCE", "memo-api")
        .env(
            "AUTH_JWKS_URI",
            "https://id.example.test/.well-known/jwks.json",
        )
        .env("AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS", "300")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run_with_stdin(mut command: Command, input: &[u8]) -> Output {
    let mut child = command.spawn().expect("preflight binary should spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin should be piped")
        .write_all(input)
        .expect("stdin write should succeed");
    drop(child.stdin.take());
    child
        .wait_with_output()
        .expect("preflight binary should complete")
}

#[test]
fn malformed_token_fails_without_echoing_sensitive_input() {
    let output = run_with_stdin(jwt_command(), MALFORMED_TOKEN.as_bytes());

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("access_token.valid=false"));
    assert!(stderr.contains("access_token.error=verification"));
    assert!(!stderr.contains(MALFORMED_TOKEN));
}

#[test]
fn token_argument_is_rejected_without_echoing_sensitive_input() {
    let mut command = jwt_command();
    command.arg(ARG_TOKEN);
    let output = run_with_stdin(command, b"");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("access_token.valid=false"));
    assert!(stderr.contains("access_token.error=usage"));
    assert!(stderr.contains("usage: verify_identity_access_token < access-token.jwt"));
    assert!(!stderr.contains(ARG_TOKEN));
}

#[test]
fn development_auth_mode_is_rejected_as_configuration_failure() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_verify_identity_access_token"));
    command
        .env_clear()
        .env("AUTH_MODE", "development")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = run_with_stdin(command, MALFORMED_TOKEN.as_bytes());
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("access_token.error=configuration"));
    assert!(!stderr.contains(MALFORMED_TOKEN));
}
