use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::FAILURE
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
    fn success_output_is_deliberately_non_identifying() {
        assert_eq!(success_output(), "access_token.valid=true");
    }
}
