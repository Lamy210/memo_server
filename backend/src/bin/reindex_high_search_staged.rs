use std::{env, process::ExitCode};

use memo_app_backend::{
    config::{AppConfig, AuthoritativeBackend, HighSearchConfig, SearchBackend},
    infrastructure::high_search_reindex_operator::run_staged_high_search_reindex,
};

const USAGE: &str = "usage:
  reindex_high_search_staged --plan --page-size <1..1000>
  reindex_high_search_staged --apply --page-size <1..1000> --confirm-all-writers-guarded --confirm-request-path-inactive";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Plan,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Command {
    mode: Mode,
    page_size: usize,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("staged HIGH search reindex failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = parse_command(env::args().skip(1).collect())?;
    let config = load_operator_config()?;
    validate_operator_config(&config)?;

    println!("mode={:?}", command.mode);
    println!("page_size={}", command.page_size);
    println!("authoritative_backend=mongodb");
    println!("search_backend=manticore");
    println!("high_search_mode=aws-kms");
    println!("request_path_cutover=none_staged_validation_only");

    if command.mode == Mode::Plan {
        println!(
            "Plan only: no KMS, MongoDB, or Manticore network operation was started. Re-run with --apply and both safety confirmations."
        );
        return Ok(());
    }

    let stats = run_staged_high_search_reindex(&config, command.page_size).await?;
    println!(
        "Staged HIGH search reindex verified: source={} projection={} projected_visited={} verified_visited={}",
        stats.source_count,
        stats.projection_count,
        stats.projected_visited,
        stats.verified_visited
    );
    println!(
        "Protected request routing remains inactive; this command did not switch application search traffic."
    );
    Ok(())
}

fn parse_command(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    let mut mode = None;
    let mut page_size = None;
    let mut confirmed_writers_guarded = false;
    let mut confirmed_request_path_inactive = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--plan" => {
                set_mode(&mut mode, Mode::Plan)?;
                index += 1;
            }
            "--apply" => {
                set_mode(&mut mode, Mode::Apply)?;
                index += 1;
            }
            "--page-size" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                page_size = Some(value.parse::<usize>()?);
                index += 2;
            }
            "--confirm-all-writers-guarded" => {
                confirmed_writers_guarded = true;
                index += 1;
            }
            "--confirm-request-path-inactive" => {
                confirmed_request_path_inactive = true;
                index += 1;
            }
            _ => return Err(USAGE.into()),
        }
    }

    let mode = mode.ok_or(USAGE)?;
    let page_size = page_size.ok_or("--page-size is required")?;

    if mode == Mode::Apply && (!confirmed_writers_guarded || !confirmed_request_path_inactive) {
        return Err(
            "apply requires --confirm-all-writers-guarded and --confirm-request-path-inactive"
                .into(),
        );
    }
    if mode == Mode::Plan && (confirmed_writers_guarded || confirmed_request_path_inactive) {
        return Err("safety confirmation flags are only valid with --apply".into());
    }

    Ok(Command { mode, page_size })
}

fn set_mode(
    mode: &mut Option<Mode>,
    candidate: Mode,
) -> Result<(), Box<dyn std::error::Error>> {
    if mode.replace(candidate).is_some() {
        return Err("choose exactly one of --plan or --apply".into());
    }
    Ok(())
}

fn load_operator_config() -> Result<AppConfig, Box<dyn std::error::Error>> {
    let mut vars: Vec<(String, String)> = env::vars()
        .filter(|(name, _)| name != "AUTH_MODE")
        .collect();
    vars.push(("AUTH_MODE".to_string(), "development".to_string()));
    Ok(AppConfig::from_vars(vars)?)
}

fn validate_operator_config(config: &AppConfig) -> Result<(), Box<dyn std::error::Error>> {
    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err("AUTHORITATIVE_BACKEND must be mongodb".into());
    }
    if config.search_backend != SearchBackend::Manticore {
        return Err("SEARCH_BACKEND must be manticore".into());
    }
    if !matches!(config.high_search, HighSearchConfig::AwsKms { .. }) {
        return Err("HIGH_SEARCH_MODE must be aws-kms".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_requires_explicit_page_size_and_no_apply_confirmations() {
        assert_eq!(
            parse_command(vec![
                "--plan".to_string(),
                "--page-size".to_string(),
                "100".to_string(),
            ])
            .unwrap(),
            Command {
                mode: Mode::Plan,
                page_size: 100,
            }
        );

        assert!(parse_command(vec!["--plan".to_string()]).is_err());
        assert!(parse_command(vec![
            "--plan".to_string(),
            "--page-size".to_string(),
            "100".to_string(),
            "--confirm-all-writers-guarded".to_string(),
        ])
        .is_err());
    }

    #[test]
    fn apply_requires_both_staging_safety_confirmations() {
        let base = vec![
            "--apply".to_string(),
            "--page-size".to_string(),
            "250".to_string(),
        ];
        assert!(parse_command(base.clone()).is_err());

        let mut complete = base;
        complete.push("--confirm-all-writers-guarded".to_string());
        complete.push("--confirm-request-path-inactive".to_string());

        assert_eq!(
            parse_command(complete).unwrap(),
            Command {
                mode: Mode::Apply,
                page_size: 250,
            }
        );
    }

    #[test]
    fn mode_is_mutually_exclusive() {
        assert!(parse_command(vec![
            "--plan".to_string(),
            "--apply".to_string(),
            "--page-size".to_string(),
            "100".to_string(),
        ])
        .is_err());
    }
}
