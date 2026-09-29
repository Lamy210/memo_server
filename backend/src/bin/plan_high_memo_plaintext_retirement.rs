use std::{env, process::ExitCode};

use memo_app_backend::{
    config::AppConfig,
    infrastructure::{
        high_memo_plaintext_retirement_operator::plan_high_memo_plaintext_retirement,
        high_memo_retirement_operator::{
            inspect_high_memo_retirement_status, HighMemoRetirementApproval,
        },
    },
};

const USAGE: &str = "usage:
  plan_high_memo_plaintext_retirement [--status]
  plan_high_memo_plaintext_retirement --plan --confirm-maintenance-window --confirm-post-cutover-backup-verified --confirm-restore-rehearsed --minimum-soak-hours <hours> --encrypted-page-size <n> --cache-scan-count <n> --expected-memo-route-generation <generation> --expected-search-route-generation <generation>";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Status,
    Plan {
        minimum_soak_hours: u64,
        encrypted_page_size: usize,
        cache_scan_count: usize,
        expected_memo_generation: i64,
        expected_search_generation: i64,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("MEMO-HIGH-1 plaintext retirement planning failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = parse_args(env::args().skip(1).collect())?;
    let config = AppConfig::from_env()?;

    match command {
        Command::Status => {
            let status = inspect_high_memo_retirement_status(&config).await?;
            println!("current.memo_route={}", status.memo_route.route);
            println!(
                "current.memo_route_generation={}",
                status.memo_route.generation
            );
            println!(
                "current.plaintext_retirement_state={}",
                status.plaintext_retirement_state
            );
            println!("current.search_route={}", status.search_route.route);
            println!(
                "current.search_route_generation={}",
                status.search_route.generation
            );
        }
        Command::Plan {
            minimum_soak_hours,
            encrypted_page_size,
            cache_scan_count,
            expected_memo_generation,
            expected_search_generation,
        } => {
            let plan = plan_high_memo_plaintext_retirement(
                &config,
                minimum_soak_hours,
                encrypted_page_size,
                cache_scan_count,
                expected_memo_generation,
                expected_search_generation,
                HighMemoRetirementApproval {
                    post_cutover_backup_verified: true,
                    restore_rehearsed: true,
                },
            )
            .await?;

            println!("plan.ready=true");
            println!("plan.retirement_state={}", plan.retirement_state);
            println!("plan.plaintext_documents={}", plan.plaintext_documents);
            println!("plan.destructive_changes=0");
            println!(
                "plan.would_require_irreversible_retirement={}",
                plan.retirement_state
                    != memo_app_backend::application::high_memo_routing::HighMemoPlaintextRetirementState::Retired
            );
            println!(
                "plan.post_retirement_audit={}",
                plan.retirement_state
                    == memo_app_backend::application::high_memo_routing::HighMemoPlaintextRetirementState::Retired
            );

            if let Some(readiness) = plan.readiness {
                println!("plan.memo_route={}", readiness.memo_route.route);
                println!(
                    "plan.memo_route_generation={}",
                    readiness.memo_route.generation
                );
                println!("plan.search_route={}", readiness.search_route.route);
                println!(
                    "plan.search_route_generation={}",
                    readiness.search_route.generation
                );
                println!(
                    "plan.encrypted_memos_verified={}",
                    readiness.encrypted_memos_verified
                );
                println!(
                    "plan.pending_projection_intents={}",
                    readiness.pending_projection_intents
                );
                println!("plan.legacy_cache_keys={}", readiness.legacy_cache_keys);
                println!(
                    "plan.legacy_search_documents={}",
                    readiness.legacy_search_documents
                );
            }
        }
    }

    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    if args.is_empty() || matches!(args.as_slice(), [flag] if flag == "--status") {
        return Ok(Command::Status);
    }

    let mut plan = false;
    let mut confirm_maintenance = false;
    let mut confirm_backup = false;
    let mut confirm_restore = false;
    let mut minimum_soak_hours = None;
    let mut encrypted_page_size = None;
    let mut cache_scan_count = None;
    let mut expected_memo_generation = None;
    let mut expected_search_generation = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--plan" => {
                plan = true;
                index += 1;
            }
            "--confirm-maintenance-window" => {
                confirm_maintenance = true;
                index += 1;
            }
            "--confirm-post-cutover-backup-verified" => {
                confirm_backup = true;
                index += 1;
            }
            "--confirm-restore-rehearsed" => {
                confirm_restore = true;
                index += 1;
            }
            "--minimum-soak-hours" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if minimum_soak_hours.is_some() {
                    return Err(USAGE.into());
                }
                minimum_soak_hours = Some(value.parse::<u64>()?);
                index += 2;
            }
            "--encrypted-page-size" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if encrypted_page_size.is_some() {
                    return Err(USAGE.into());
                }
                encrypted_page_size = Some(value.parse::<usize>()?);
                index += 2;
            }
            "--cache-scan-count" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if cache_scan_count.is_some() {
                    return Err(USAGE.into());
                }
                cache_scan_count = Some(value.parse::<usize>()?);
                index += 2;
            }
            "--expected-memo-route-generation" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if expected_memo_generation.is_some() {
                    return Err(USAGE.into());
                }
                let generation = value.parse::<i64>()?;
                if generation < 0 {
                    return Err("expected memo route generation must be non-negative".into());
                }
                expected_memo_generation = Some(generation);
                index += 2;
            }
            "--expected-search-route-generation" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if expected_search_generation.is_some() {
                    return Err(USAGE.into());
                }
                let generation = value.parse::<i64>()?;
                if generation < 0 {
                    return Err("expected search route generation must be non-negative".into());
                }
                expected_search_generation = Some(generation);
                index += 2;
            }
            _ => return Err(USAGE.into()),
        }
    }

    if !plan
        || !confirm_maintenance
        || !confirm_backup
        || !confirm_restore
        || minimum_soak_hours.is_none()
        || encrypted_page_size.is_none()
        || cache_scan_count.is_none()
        || expected_memo_generation.is_none()
        || expected_search_generation.is_none()
    {
        return Err(USAGE.into());
    }

    Ok(Command::Plan {
        minimum_soak_hours: minimum_soak_hours.unwrap(),
        encrypted_page_size: encrypted_page_size.unwrap(),
        cache_scan_count: cache_scan_count.unwrap(),
        expected_memo_generation: expected_memo_generation.unwrap(),
        expected_search_generation: expected_search_generation.unwrap(),
    })
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
    fn planning_requires_every_explicit_confirmation() {
        let valid = vec![
            "--plan".into(),
            "--confirm-maintenance-window".into(),
            "--confirm-post-cutover-backup-verified".into(),
            "--confirm-restore-rehearsed".into(),
            "--minimum-soak-hours".into(),
            "168".into(),
            "--encrypted-page-size".into(),
            "500".into(),
            "--cache-scan-count".into(),
            "1000".into(),
            "--expected-memo-route-generation".into(),
            "5".into(),
            "--expected-search-route-generation".into(),
            "9".into(),
        ];

        assert_eq!(
            parse_args(valid.clone()).unwrap(),
            Command::Plan {
                minimum_soak_hours: 168,
                encrypted_page_size: 500,
                cache_scan_count: 1000,
                expected_memo_generation: 5,
                expected_search_generation: 9,
            }
        );

        for required in [
            "--confirm-maintenance-window",
            "--confirm-post-cutover-backup-verified",
            "--confirm-restore-rehearsed",
        ] {
            let missing = valid
                .iter()
                .filter(|value| value.as_str() != required)
                .cloned()
                .collect();
            assert!(parse_args(missing).is_err(), "{required}");
        }
    }
}
