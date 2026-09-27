use std::{env, process::ExitCode};

use memo_app_backend::{
    config::AppConfig,
    infrastructure::high_memo_migration_operator::{
        plan_high_memo_migration, run_high_memo_migration,
    },
};

const USAGE: &str = "usage:
  migrate_high_memo_staged [--plan]
  migrate_high_memo_staged --apply --confirm-staging-reset --page-size <1..=1000>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Plan,
    Apply { page_size: usize },
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("MEMO-HIGH-1 staged migration failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = parse_args(env::args().skip(1).collect())?;
    let config = AppConfig::from_env()?;

    match command {
        Command::Plan => {
            let plan = plan_high_memo_migration(&config).await?;
            println!("plan.source_count={}", plan.source_count);
            println!("plan.staged_count={}", plan.staged_count);
            println!("plan.authoritative_cutover=false");
            println!("plan.staging_reset_on_apply=true");
        }
        Command::Apply { page_size } => {
            let stats = run_high_memo_migration(&config, page_size).await?;
            println!("migration.source_count={}", stats.source_count);
            println!("migration.staged_count={}", stats.staged_count);
            println!("migration.migrated_visited={}", stats.migrated_visited);
            println!("migration.verified_visited={}", stats.verified_visited);
            println!("migration.inserted_verified={}", stats.inserted_verified);
            println!(
                "migration.already_present_verified={}",
                stats.already_present_verified
            );
            println!("migration.authoritative_cutover=false");
        }
    }

    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    if args.is_empty() || matches!(args.as_slice(), [flag] if flag == "--plan") {
        return Ok(Command::Plan);
    }

    let mut apply = false;
    let mut confirmed_reset = false;
    let mut page_size = None;
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
            "--confirm-staging-reset" => {
                if confirmed_reset {
                    return Err(USAGE.into());
                }
                confirmed_reset = true;
                index += 1;
            }
            "--page-size" => {
                let raw = args.get(index + 1).ok_or(USAGE)?;
                if page_size.is_some() {
                    return Err(USAGE.into());
                }
                page_size = Some(raw.parse::<usize>()?);
                index += 2;
            }
            _ => return Err(USAGE.into()),
        }
    }

    if !apply || !confirmed_reset {
        return Err(USAGE.into());
    }

    Ok(Command::Apply {
        page_size: page_size.ok_or(USAGE)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_is_the_non_destructive_default() {
        assert_eq!(parse_args(Vec::new()).unwrap(), Command::Plan);
        assert_eq!(parse_args(vec!["--plan".into()]).unwrap(), Command::Plan);
    }

    #[test]
    fn apply_requires_explicit_reset_confirmation_and_page_size() {
        assert!(parse_args(vec!["--apply".into()]).is_err());
        assert!(parse_args(vec!["--apply".into(), "--confirm-staging-reset".into(),]).is_err());

        assert_eq!(
            parse_args(vec![
                "--apply".into(),
                "--confirm-staging-reset".into(),
                "--page-size".into(),
                "500".into(),
            ])
            .unwrap(),
            Command::Apply { page_size: 500 }
        );
    }
}
