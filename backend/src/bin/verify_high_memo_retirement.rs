use std::{env, process::ExitCode};

use memo_app_backend::{
    config::AppConfig,
    infrastructure::high_memo_retirement_operator::{
        inspect_high_memo_retirement_status, verify_high_memo_retirement_readiness,
        HighMemoRetirementApproval,
    },
};

const USAGE: &str = "usage:
  verify_high_memo_retirement [--status]
  verify_high_memo_retirement --verify --confirm-maintenance-window --confirm-post-cutover-backup-verified --confirm-restore-rehearsed --minimum-soak-hours <hours> --cache-scan-count <n> --expected-memo-route-generation <generation> --expected-search-route-generation <generation>";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Status,
    Verify {
        minimum_soak_hours: u64,
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
            eprintln!("MEMO-HIGH-1 retirement readiness failed: {error}");
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
            println!("current.search_route={}", status.search_route.route);
            println!(
                "current.search_route_generation={}",
                status.search_route.generation
            );
            match status.memo_route_changed_at_ms {
                Some(value) => println!("current.memo_route_changed_at_ms={value}"),
                None => println!("current.memo_route_changed_at_ms=unknown"),
            }
        }
        Command::Verify {
            minimum_soak_hours,
            cache_scan_count,
            expected_memo_generation,
            expected_search_generation,
        } => {
            let report = verify_high_memo_retirement_readiness(
                &config,
                minimum_soak_hours,
                cache_scan_count,
                expected_memo_generation,
                expected_search_generation,
                HighMemoRetirementApproval {
                    post_cutover_backup_verified: true,
                    restore_rehearsed: true,
                },
            )
            .await?;

            println!("readiness.ready=true");
            println!("readiness.memo_route={}", report.memo_route.route);
            println!(
                "readiness.memo_route_generation={}",
                report.memo_route.generation
            );
            println!("readiness.search_route={}", report.search_route.route);
            println!(
                "readiness.search_route_generation={}",
                report.search_route.generation
            );
            println!(
                "readiness.memo_route_changed_at_ms={}",
                report.memo_route_changed_at_ms
            );
            println!("readiness.observed_at_ms={}", report.observed_at_ms);
            println!(
                "readiness.minimum_soak_hours={}",
                report.minimum_soak_hours
            );
            println!("readiness.observed_soak_hours={}", report.observed_soak_hours);
            println!(
                "readiness.pending_projection_intents={}",
                report.pending_projection_intents
            );
            println!("readiness.legacy_cache_keys={}", report.legacy_cache_keys);
            println!(
                "readiness.legacy_search_documents={}",
                report.legacy_search_documents
            );
        }
    }

    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    if args.is_empty() || matches!(args.as_slice(), [flag] if flag == "--status") {
        return Ok(Command::Status);
    }

    let mut verify = false;
    let mut confirm_maintenance = false;
    let mut confirm_backup = false;
    let mut confirm_restore = false;
    let mut minimum_soak_hours = None;
    let mut cache_scan_count = None;
    let mut expected_memo_generation = None;
    let mut expected_search_generation = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--verify" => {
                verify = true;
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

    if !verify
        || !confirm_maintenance
        || !confirm_backup
        || !confirm_restore
        || minimum_soak_hours.is_none()
        || cache_scan_count.is_none()
        || expected_memo_generation.is_none()
        || expected_search_generation.is_none()
    {
        return Err(USAGE.into());
    }

    Ok(Command::Verify {
        minimum_soak_hours: minimum_soak_hours.unwrap(),
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
    fn verification_requires_every_explicit_confirmation() {
        let valid = vec![
            "--verify".into(),
            "--confirm-maintenance-window".into(),
            "--confirm-post-cutover-backup-verified".into(),
            "--confirm-restore-rehearsed".into(),
            "--minimum-soak-hours".into(),
            "168".into(),
            "--cache-scan-count".into(),
            "1000".into(),
            "--expected-memo-route-generation".into(),
            "5".into(),
            "--expected-search-route-generation".into(),
            "9".into(),
        ];

        assert_eq!(
            parse_args(valid.clone()).unwrap(),
            Command::Verify {
                minimum_soak_hours: 168,
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
