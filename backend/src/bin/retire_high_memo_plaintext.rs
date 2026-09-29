use std::{env, process::ExitCode};

use memo_app_backend::{
    config::AppConfig,
    infrastructure::{
        high_memo_plaintext_retirement_delete_operator::{
            retire_high_memo_plaintext, HighMemoPlaintextRetirementRequest,
        },
        high_memo_retirement_operator::{
            inspect_high_memo_retirement_status, HighMemoRetirementApproval,
        },
    },
};

const USAGE: &str = "usage:
  retire_high_memo_plaintext [--status]
  retire_high_memo_plaintext --apply --confirm-irrevocable-plaintext-delete --confirm-maintenance-window --confirm-post-cutover-backup-verified --confirm-restore-rehearsed --confirm-legacy-backup-retention-reviewed --minimum-soak-hours <hours> --encrypted-page-size <n> --cache-scan-count <n> --expected-memo-route-generation <generation> --expected-search-route-generation <generation> --expected-plaintext-documents <count>";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Status,
    Apply(HighMemoPlaintextRetirementRequest),
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("MEMO-HIGH-1 plaintext retirement failed: {error}");
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
        Command::Apply(request) => {
            let report = retire_high_memo_plaintext(&config, request).await?;

            println!("retirement.completed=true");
            println!(
                "retirement.initial_state={}",
                report.initial_retirement_state
            );
            println!("retirement.final_state={}", report.final_retirement_state);
            println!(
                "retirement.observed_plaintext_documents={}",
                report.observed_plaintext_documents
            );
            println!(
                "retirement.deleted_plaintext_documents={}",
                report.deleted_plaintext_documents
            );
            println!("retirement.remaining_plaintext_documents=0");
            if let Some(readiness) = report.readiness {
                println!(
                    "retirement.memo_route_generation={}",
                    readiness.memo_route.generation
                );
                println!(
                    "retirement.search_route_generation={}",
                    readiness.search_route.generation
                );
                println!(
                    "retirement.encrypted_memos_verified={}",
                    readiness.encrypted_memos_verified
                );
                println!(
                    "retirement.pending_projection_intents={}",
                    readiness.pending_projection_intents
                );
                println!(
                    "retirement.legacy_cache_keys={}",
                    readiness.legacy_cache_keys
                );
                println!(
                    "retirement.legacy_search_documents={}",
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

    let mut apply = false;
    let mut confirm_delete = false;
    let mut confirm_maintenance = false;
    let mut confirm_backup = false;
    let mut confirm_restore = false;
    let mut confirm_legacy_backup_retention = false;
    let mut minimum_soak_hours = None;
    let mut encrypted_page_size = None;
    let mut cache_scan_count = None;
    let mut expected_memo_generation = None;
    let mut expected_search_generation = None;
    let mut expected_plaintext_documents = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--apply" => {
                if apply {
                    return Err(USAGE.into());
                }
                apply = true;
                index += 1;
            }
            "--confirm-irrevocable-plaintext-delete" => {
                if confirm_delete {
                    return Err(USAGE.into());
                }
                confirm_delete = true;
                index += 1;
            }
            "--confirm-maintenance-window" => {
                if confirm_maintenance {
                    return Err(USAGE.into());
                }
                confirm_maintenance = true;
                index += 1;
            }
            "--confirm-post-cutover-backup-verified" => {
                if confirm_backup {
                    return Err(USAGE.into());
                }
                confirm_backup = true;
                index += 1;
            }
            "--confirm-restore-rehearsed" => {
                if confirm_restore {
                    return Err(USAGE.into());
                }
                confirm_restore = true;
                index += 1;
            }
            "--confirm-legacy-backup-retention-reviewed" => {
                if confirm_legacy_backup_retention {
                    return Err(USAGE.into());
                }
                confirm_legacy_backup_retention = true;
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
            "--expected-plaintext-documents" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if expected_plaintext_documents.is_some() {
                    return Err(USAGE.into());
                }
                expected_plaintext_documents = Some(value.parse::<u64>()?);
                index += 2;
            }
            _ => return Err(USAGE.into()),
        }
    }

    if !apply
        || !confirm_delete
        || !confirm_maintenance
        || !confirm_backup
        || !confirm_restore
        || !confirm_legacy_backup_retention
        || minimum_soak_hours.is_none()
        || encrypted_page_size.is_none()
        || cache_scan_count.is_none()
        || expected_memo_generation.is_none()
        || expected_search_generation.is_none()
        || expected_plaintext_documents.is_none()
    {
        return Err(USAGE.into());
    }

    Ok(Command::Apply(HighMemoPlaintextRetirementRequest {
        minimum_soak_hours: minimum_soak_hours.unwrap(),
        encrypted_page_size: encrypted_page_size.unwrap(),
        cache_scan_count: cache_scan_count.unwrap(),
        expected_memo_generation: expected_memo_generation.unwrap(),
        expected_search_generation: expected_search_generation.unwrap(),
        expected_plaintext_documents: expected_plaintext_documents.unwrap(),
        legacy_backup_retention_reviewed: true,
        approval: HighMemoRetirementApproval {
            post_cutover_backup_verified: true,
            restore_rehearsed: true,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_args() -> Vec<String> {
        vec![
            "--apply".into(),
            "--confirm-irrevocable-plaintext-delete".into(),
            "--confirm-maintenance-window".into(),
            "--confirm-post-cutover-backup-verified".into(),
            "--confirm-restore-rehearsed".into(),
            "--confirm-legacy-backup-retention-reviewed".into(),
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
            "--expected-plaintext-documents".into(),
            "42".into(),
        ]
    }

    #[test]
    fn status_is_non_destructive_default() {
        assert_eq!(parse_args(Vec::new()).unwrap(), Command::Status);
        assert_eq!(
            parse_args(vec!["--status".into()]).unwrap(),
            Command::Status
        );
    }

    #[test]
    fn apply_requires_irrevocable_confirmation_and_exact_planned_count() {
        let expected = HighMemoPlaintextRetirementRequest {
            minimum_soak_hours: 168,
            encrypted_page_size: 500,
            cache_scan_count: 1000,
            expected_memo_generation: 5,
            expected_search_generation: 9,
            expected_plaintext_documents: 42,
            legacy_backup_retention_reviewed: true,
            approval: HighMemoRetirementApproval {
                post_cutover_backup_verified: true,
                restore_rehearsed: true,
            },
        };
        assert_eq!(parse_args(valid_args()).unwrap(), Command::Apply(expected));

        for required in [
            "--confirm-irrevocable-plaintext-delete",
            "--confirm-maintenance-window",
            "--confirm-post-cutover-backup-verified",
            "--confirm-restore-rehearsed",
            "--confirm-legacy-backup-retention-reviewed",
            "--expected-plaintext-documents",
        ] {
            let args = valid_args()
                .into_iter()
                .filter(|value| value != required)
                .collect();
            assert!(parse_args(args).is_err(), "{required}");
        }
    }
}
