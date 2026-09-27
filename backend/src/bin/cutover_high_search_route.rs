use std::{env, fs, path::PathBuf, process::ExitCode};

use memo_app_backend::{
    config::AppConfig,
    infrastructure::{
        high_search_cutover_approval::{
            parse_high_search_cutover_approval_json, MAX_HIGH_SEARCH_CUTOVER_APPROVAL_FILE_BYTES,
        },
        high_search_route_operator::{
            inspect_high_search_query_route, run_legacy_high_search_rollback,
            run_protected_high_search_cutover,
        },
    },
};

const USAGE: &str = "usage:
  cutover_high_search_route [--status]
  cutover_high_search_route --apply-protected --confirm-protected-cutover --approval <approval.json> --page-size <n> --expected-route-generation <generation>
  cutover_high_search_route --apply-legacy --confirm-legacy-rollback --expected-route-generation <generation>";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Status,
    Protected {
        approval: PathBuf,
        page_size: usize,
        expected_generation: i64,
    },
    Legacy {
        expected_generation: i64,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("HIGH search route operation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = parse_args(env::args().skip(1).collect())?;
    let config = AppConfig::from_env()?;

    match command {
        Command::Status => {
            let route = inspect_high_search_query_route(&config).await?;
            print_route("current", route);
        }
        Command::Protected {
            approval,
            page_size,
            expected_generation,
        } => {
            let approval = read_cutover_approval(&approval)?;
            let report = run_protected_high_search_cutover(
                &config,
                page_size,
                expected_generation,
                &approval,
            )
            .await?;

            println!("cutover.approval_id={}", approval.approval_id);
            print_route("cutover.previous", report.previous);
            print_route("cutover.current", report.current);
            if let Some(stats) = report.reindex {
                println!("cutover.reindex.source_count={}", stats.source_count);
                println!("cutover.reindex.projection_count={}", stats.projection_count);
                println!("cutover.reindex.projected_visited={}", stats.projected_visited);
                println!("cutover.reindex.verified_visited={}", stats.verified_visited);
            }
        }
        Command::Legacy {
            expected_generation,
        } => {
            let report = run_legacy_high_search_rollback(&config, expected_generation).await?;
            print_route("rollback.previous", report.previous);
            print_route("rollback.current", report.current);
        }
    }

    Ok(())
}

fn read_cutover_approval(
    path: &PathBuf,
) -> Result<
    memo_app_backend::infrastructure::high_search_cutover_approval::HighSearchCutoverApproval,
    Box<dyn std::error::Error>,
> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("failed to inspect cutover approval file: {error}"))?;
    if metadata.len() > MAX_HIGH_SEARCH_CUTOVER_APPROVAL_FILE_BYTES {
        return Err(format!(
            "cutover approval exceeds the {} byte safety limit",
            MAX_HIGH_SEARCH_CUTOVER_APPROVAL_FILE_BYTES
        )
        .into());
    }

    let input = fs::read_to_string(path)
        .map_err(|error| format!("failed to read cutover approval file as UTF-8: {error}"))?;
    Ok(parse_high_search_cutover_approval_json(&input)?)
}

fn print_route(
    label: &str,
    route: memo_app_backend::application::high_search_routing::HighSearchQueryRouteSnapshot,
) {
    println!("{label}.route={}", route.route);
    println!("{label}.generation={}", route.generation);
}

fn parse_args(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    if args.is_empty() || matches!(args.as_slice(), [flag] if flag == "--status") {
        return Ok(Command::Status);
    }

    let mut apply_protected = false;
    let mut apply_legacy = false;
    let mut confirm_protected = false;
    let mut confirm_legacy = false;
    let mut approval = None;
    let mut page_size = None;
    let mut expected_generation = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--apply-protected" => {
                apply_protected = true;
                index += 1;
            }
            "--apply-legacy" => {
                apply_legacy = true;
                index += 1;
            }
            "--confirm-protected-cutover" => {
                confirm_protected = true;
                index += 1;
            }
            "--confirm-legacy-rollback" => {
                confirm_legacy = true;
                index += 1;
            }
            "--approval" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if value.trim().is_empty() || approval.is_some() {
                    return Err(USAGE.into());
                }
                approval = Some(PathBuf::from(value));
                index += 2;
            }
            "--page-size" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if page_size.is_some() {
                    return Err(USAGE.into());
                }
                page_size = Some(value.parse::<usize>()?);
                index += 2;
            }
            "--expected-route-generation" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if expected_generation.is_some() {
                    return Err(USAGE.into());
                }
                let generation = value.parse::<i64>()?;
                if generation < 0 {
                    return Err("expected route generation must be non-negative".into());
                }
                expected_generation = Some(generation);
                index += 2;
            }
            _ => return Err(USAGE.into()),
        }
    }

    match (apply_protected, apply_legacy) {
        (true, false) => {
            if !confirm_protected
                || confirm_legacy
                || approval.is_none()
                || page_size.is_none()
                || expected_generation.is_none()
            {
                return Err(USAGE.into());
            }
            Ok(Command::Protected {
                approval: approval.unwrap(),
                page_size: page_size.unwrap(),
                expected_generation: expected_generation.unwrap(),
            })
        }
        (false, true) => {
            if !confirm_legacy
                || confirm_protected
                || approval.is_some()
                || page_size.is_some()
                || expected_generation.is_none()
            {
                return Err(USAGE.into());
            }
            Ok(Command::Legacy {
                expected_generation: expected_generation.unwrap(),
            })
        }
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_non_destructive_default() {
        assert_eq!(parse_args(Vec::new()).unwrap(), Command::Status);
        assert_eq!(
            parse_args(vec!["--status".into()]).unwrap(),
            Command::Status
        );
    }

    #[test]
    fn protected_cutover_requires_approval_confirmation_and_generation() {
        assert!(parse_args(vec!["--apply-protected".into()]).is_err());

        assert_eq!(
            parse_args(vec![
                "--apply-protected".into(),
                "--confirm-protected-cutover".into(),
                "--approval".into(),
                "approval.json".into(),
                "--page-size".into(),
                "500".into(),
                "--expected-route-generation".into(),
                "7".into(),
            ])
            .unwrap(),
            Command::Protected {
                approval: PathBuf::from("approval.json"),
                page_size: 500,
                expected_generation: 7,
            }
        );
    }

    #[test]
    fn rollback_is_separate_and_does_not_accept_cutover_inputs() {
        assert_eq!(
            parse_args(vec![
                "--apply-legacy".into(),
                "--confirm-legacy-rollback".into(),
                "--expected-route-generation".into(),
                "8".into(),
            ])
            .unwrap(),
            Command::Legacy {
                expected_generation: 8,
            }
        );

        assert!(parse_args(vec![
            "--apply-legacy".into(),
            "--confirm-legacy-rollback".into(),
            "--expected-route-generation".into(),
            "8".into(),
            "--approval".into(),
            "approval.json".into(),
        ])
        .is_err());
    }
}
