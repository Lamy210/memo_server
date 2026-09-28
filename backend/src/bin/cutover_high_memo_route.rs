use std::{env, process::ExitCode};

use memo_app_backend::{
    config::AppConfig,
    infrastructure::high_memo_route_operator::{
        inspect_high_memo_cutover_routes, run_encrypted_high_memo_cutover,
        HighMemoRouteCutoverApproval,
    },
};

const USAGE: &str = "usage:
  cutover_high_memo_route [--status]
  cutover_high_memo_route --apply-encrypted --confirm-encrypted-cutover --confirm-all-replicas-encrypted-ready --confirm-plaintext-backup-verified --confirm-no-automatic-rollback --page-size <n> --cache-scan-count <n> --expected-memo-route-generation <generation> --expected-search-route-generation <generation>";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Status,
    ApplyEncrypted {
        page_size: usize,
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
            eprintln!("MEMO-HIGH-1 route operation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = parse_args(env::args().skip(1).collect())?;
    let config = AppConfig::from_env()?;

    match command {
        Command::Status => {
            let (memo, search) = inspect_high_memo_cutover_routes(&config).await?;
            println!("current.memo_route={}", memo.route);
            println!("current.memo_route_generation={}", memo.generation);
            println!("current.search_route={}", search.route);
            println!("current.search_route_generation={}", search.generation);
        }
        Command::ApplyEncrypted {
            page_size,
            cache_scan_count,
            expected_memo_generation,
            expected_search_generation,
        } => {
            let report = run_encrypted_high_memo_cutover(
                &config,
                page_size,
                cache_scan_count,
                expected_memo_generation,
                expected_search_generation,
                HighMemoRouteCutoverApproval {
                    all_replicas_encrypted_ready: true,
                    plaintext_backup_verified: true,
                    no_automatic_rollback_accepted: true,
                },
            )
            .await?;

            println!(
                "cutover.previous_memo_route={}",
                report.previous_memo_route.route
            );
            println!(
                "cutover.previous_memo_route_generation={}",
                report.previous_memo_route.generation
            );
            println!(
                "cutover.current_memo_route={}",
                report.current_memo_route.route
            );
            println!(
                "cutover.current_memo_route_generation={}",
                report.current_memo_route.generation
            );
            println!("cutover.search_route={}", report.search_route.route);
            println!(
                "cutover.search_route_generation={}",
                report.search_route.generation
            );
            println!(
                "cutover.legacy_search_purged={}",
                report.legacy_search_purged
            );

            if let Some(stats) = report.migration {
                println!("cutover.migration.source_count={}", stats.source_count);
                println!("cutover.migration.staged_count={}", stats.staged_count);
                println!(
                    "cutover.migration.migrated_visited={}",
                    stats.migrated_visited
                );
                println!(
                    "cutover.migration.verified_visited={}",
                    stats.verified_visited
                );
            }
            if let Some(stats) = report.legacy_cache_purge {
                println!(
                    "cutover.legacy_cache.scanned_candidates={}",
                    stats.scanned_candidates
                );
                println!("cutover.legacy_cache.legacy_keys={}", stats.legacy_keys);
                println!("cutover.legacy_cache.deleted_keys={}", stats.deleted_keys);
            }
        }
    }

    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    if args.is_empty() || matches!(args.as_slice(), [flag] if flag == "--status") {
        return Ok(Command::Status);
    }

    let mut apply_encrypted = false;
    let mut confirm_cutover = false;
    let mut confirm_replicas = false;
    let mut confirm_backup = false;
    let mut confirm_no_rollback = false;
    let mut page_size = None;
    let mut cache_scan_count = None;
    let mut expected_memo_generation = None;
    let mut expected_search_generation = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--apply-encrypted" => {
                apply_encrypted = true;
                index += 1;
            }
            "--confirm-encrypted-cutover" => {
                confirm_cutover = true;
                index += 1;
            }
            "--confirm-all-replicas-encrypted-ready" => {
                confirm_replicas = true;
                index += 1;
            }
            "--confirm-plaintext-backup-verified" => {
                confirm_backup = true;
                index += 1;
            }
            "--confirm-no-automatic-rollback" => {
                confirm_no_rollback = true;
                index += 1;
            }
            "--page-size" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if page_size.is_some() {
                    return Err(USAGE.into());
                }
                page_size = Some(value.parse::<usize>()?);
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

    if !apply_encrypted
        || !confirm_cutover
        || !confirm_replicas
        || !confirm_backup
        || !confirm_no_rollback
        || page_size.is_none()
        || cache_scan_count.is_none()
        || expected_memo_generation.is_none()
        || expected_search_generation.is_none()
    {
        return Err(USAGE.into());
    }

    Ok(Command::ApplyEncrypted {
        page_size: page_size.unwrap(),
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
    fn encrypted_cutover_requires_every_explicit_confirmation() {
        let valid = vec![
            "--apply-encrypted".into(),
            "--confirm-encrypted-cutover".into(),
            "--confirm-all-replicas-encrypted-ready".into(),
            "--confirm-plaintext-backup-verified".into(),
            "--confirm-no-automatic-rollback".into(),
            "--page-size".into(),
            "500".into(),
            "--cache-scan-count".into(),
            "1000".into(),
            "--expected-memo-route-generation".into(),
            "4".into(),
            "--expected-search-route-generation".into(),
            "8".into(),
        ];
        assert_eq!(
            parse_args(valid.clone()).unwrap(),
            Command::ApplyEncrypted {
                page_size: 500,
                cache_scan_count: 1000,
                expected_memo_generation: 4,
                expected_search_generation: 8,
            }
        );

        for required in [
            "--confirm-encrypted-cutover",
            "--confirm-all-replicas-encrypted-ready",
            "--confirm-plaintext-backup-verified",
            "--confirm-no-automatic-rollback",
        ] {
            let missing = valid
                .iter()
                .filter(|value| value.as_str() != required)
                .cloned()
                .collect();
            assert!(parse_args(missing).is_err(), "{required}");
        }
    }

    #[test]
    fn encrypted_cutover_rejects_missing_or_negative_generations() {
        assert!(parse_args(vec!["--apply-encrypted".into()]).is_err());

        let args = vec![
            "--apply-encrypted".into(),
            "--confirm-encrypted-cutover".into(),
            "--confirm-all-replicas-encrypted-ready".into(),
            "--confirm-plaintext-backup-verified".into(),
            "--confirm-no-automatic-rollback".into(),
            "--page-size".into(),
            "500".into(),
            "--cache-scan-count".into(),
            "1000".into(),
            "--expected-memo-route-generation".into(),
            "-1".into(),
            "--expected-search-route-generation".into(),
            "8".into(),
        ];
        assert!(parse_args(args).is_err());
    }
}
