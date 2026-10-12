use std::{
    env,
    error::Error,
    fmt,
    io::{self, Read},
    process::ExitCode,
};

use memo_app_backend::{
    config::{AppConfig, AuthConfig},
    infrastructure::auth::AuthService,
};

const USAGE: &str = "usage: verify_identity_access_token < access-token.jwt";
// The resource server independently enforces a 16 KiB compact JWT limit.
// Permit one terminal CRLF at the stdin boundary without widening that token limit.
const IDENTITY_ACCESS_TOKEN_MAX_BYTES: usize = 16 * 1024;
const IDENTITY_ACCESS_TOKEN_INPUT_MAX_BYTES: usize = IDENTITY_ACCESS_TOKEN_MAX_BYTES + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreflightError {
    Usage,
    Input,
    Configuration,
    Verification,
}

impl PreflightError {
    fn code(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Input => "input",
            Self::Configuration => "configuration",
            Self::Verification => "verification",
        }
    }
}

impl fmt::Display for PreflightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Error for PreflightError {}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => {
            println!("{}", success_output());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("access_token.valid=false");
            eprintln!("access_token.error={}", error.code());
            if error == PreflightError::Usage {
                eprintln!("{USAGE}");
            }
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), PreflightError> {
    parse_args(env::args().skip(1).collect())?;

    let input = read_stdin_token_bytes()?;
    let token = token_from_stdin_bytes(&input)?;

    let config = AppConfig::from_env().map_err(|_| PreflightError::Configuration)?;
    let auth_config = match config.auth {
        AuthConfig::Development => return Err(PreflightError::Configuration),
        jwt @ AuthConfig::Jwt { .. } => jwt,
    };

    let auth = AuthService::new(auth_config).map_err(|_| PreflightError::Configuration)?;
    auth.authenticate(Some(token), None)
        .await
        .map_err(|_| PreflightError::Verification)?;

    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<(), PreflightError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(PreflightError::Usage)
    }
}

fn read_stdin_token_bytes() -> Result<Vec<u8>, PreflightError> {
    let stdin = io::stdin();
    let mut reader = stdin
        .lock()
        .take((IDENTITY_ACCESS_TOKEN_INPUT_MAX_BYTES + 1) as u64);
    let mut input = Vec::with_capacity(IDENTITY_ACCESS_TOKEN_INPUT_MAX_BYTES.min(4096));
    reader
        .read_to_end(&mut input)
        .map_err(|_| PreflightError::Input)?;

    if input.len() > IDENTITY_ACCESS_TOKEN_INPUT_MAX_BYTES {
        return Err(PreflightError::Input);
    }

    Ok(input)
}

fn token_from_stdin_bytes(input: &[u8]) -> Result<&str, PreflightError> {
    if input.is_empty() || input.len() > IDENTITY_ACCESS_TOKEN_INPUT_MAX_BYTES {
        return Err(PreflightError::Input);
    }

    let input = std::str::from_utf8(input).map_err(|_| PreflightError::Input)?;
    let token = if let Some(value) = input.strip_suffix("\r\n") {
        value
    } else if let Some(value) = input.strip_suffix('\n') {
        value
    } else {
        input
    };

    if token.is_empty()
        || token.len() > IDENTITY_ACCESS_TOKEN_MAX_BYTES
        || token.contains('\r')
        || token.contains('\n')
    {
        return Err(PreflightError::Input);
    }

    Ok(token)
}

fn success_output() -> &'static str {
    "access_token.valid=true"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_accepts_no_arguments_only() {
        assert!(parse_args(Vec::new()).is_ok());
        assert!(parse_args(vec!["token-value".into()]).is_err());
        assert!(parse_args(vec!["--token".into(), "token-value".into()]).is_err());
        assert!(parse_args(vec!["--help".into()]).is_err());
    }

    #[test]
    fn stdin_token_accepts_one_terminal_line_ending_without_trimming_token_bytes() {
        assert_eq!(
            token_from_stdin_bytes(b"header.payload.signature\n").unwrap(),
            "header.payload.signature"
        );
        assert_eq!(
            token_from_stdin_bytes(b"header.payload.signature\r\n").unwrap(),
            "header.payload.signature"
        );
        assert_eq!(
            token_from_stdin_bytes(b" header.payload.signature\n").unwrap(),
            " header.payload.signature"
        );
        assert_eq!(
            token_from_stdin_bytes(b"header.payload.signature \n").unwrap(),
            "header.payload.signature "
        );
    }

    #[test]
    fn stdin_token_rejects_empty_multiple_lines_non_utf8_and_oversized_input() {
        assert!(token_from_stdin_bytes(b"").is_err());
        assert!(token_from_stdin_bytes(b"\n").is_err());
        assert!(token_from_stdin_bytes(b"header.payload.signature\nsecond-line").is_err());
        assert!(token_from_stdin_bytes(&[0xff, 0xfe, 0xfd]).is_err());

        let oversized = vec![b'a'; IDENTITY_ACCESS_TOKEN_INPUT_MAX_BYTES + 1];
        assert!(token_from_stdin_bytes(&oversized).is_err());
    }

    #[test]
    fn sensitive_input_is_zeroized_before_release() {
        let mut input = b"header.payload.signature\n".to_vec();
        zeroize_input(&mut input);
        assert!(input.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn success_output_is_deliberately_non_identifying() {
        assert_eq!(success_output(), "access_token.valid=true");
    }
}
