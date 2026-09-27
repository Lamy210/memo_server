use std::{env, process::ExitCode};

use memo_app_backend::{
    application::high_search_routing::HighSearchQueryRoute,
    infrastructure::high_search_maintenance_mongodb::{
        HighSearchMaintenanceMode, MongoHighSearchMaintenanceRecovery,
    },
};

const USAGE: &str = "usage:
  recover_high_search_maintenance [--status]
  recover_high_search_maintenance --apply --confirm-app-stopped --expected-writer-epoch <epoch> --expected-query-route <legacy|protected> --expected-query-route-generation <generation> [--expected-holder-token <token>]";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Status,
    Apply {
        expected_writer_epoch: i64,
        expected_query_route: HighSearchQueryRoute,
        expected_query_route_generation: i64,
        expected_holder_token: Option<String>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("HIGH search maintenance recovery failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = parse_command(env::args().skip(1).collect())?;
    let uri = required_env("MONGODB_URI")?;
    let database = required_env("MONGODB_DATABASE")?;
    let recovery = MongoHighSearchMaintenanceRecovery::connect(&uri, &database).await?;
    let status = recovery.inspect().await?;

    print_status("current", &status);

    let Command::Apply {
        expected_writer_epoch,
        expected_query_route,
        expected_query_route_generation,
        expected_holder_token,
    } = command
    else {
        return Ok(());
    };

    if status.writer_epoch() != expected_writer_epoch {
        return Err(format!(
            "writer epoch changed: expected {expected_writer_epoch}, observed {}; inspect again",
            status.writer_epoch()
        )
        .into());
    }
    if status.query_route() != expected_query_route {
        return Err(format!(
            "query route changed: expected {expected_query_route}, observed {}; inspect again",
            status.query_route()
        )
        .into());
    }
    if status.query_route_generation() != expected_query_route_generation {
        return Err(format!(
            "query route generation changed: expected {expected_query_route_generation}, observed {}; inspect again",
            status.query_route_generation()
        )
        .into());
    }

    match status.mode() {
        HighSearchMaintenanceMode::Open => {
            if expected_holder_token.is_some() {
                return Err(
                    "--expected-holder-token must be omitted while the maintenance gate is open"
                        .into(),
                );
            }
            if status.active_writer_leases() == 0 && status.active_query_leases() == 0 {
                return Err(
                    "nothing to recover: gate is open and no writer or protected query leases exist"
                        .into(),
                );
            }
        }
        HighSearchMaintenanceMode::Maintenance => {
            let expected = expected_holder_token.as_deref().ok_or(
                "--expected-holder-token is required while the maintenance gate is active",
            )?;
            if status.holder_token() != Some(expected) {
                return Err(
                    "maintenance holder token changed; inspect again before recovery".into(),
                );
            }
        }
    }

    let recovered = recovery.recover_stale_state(&status).await?;
    print_status("recovered", &recovered);
    Ok(())
}

fn parse_command(args: Vec<String>) -> Result<Command, Box<dyn std::error::Error>> {
    if args.is_empty() || matches!(args.as_slice(), [flag] if flag == "--status") {
        return Ok(Command::Status);
    }

    let mut apply = false;
    let mut confirmed_stopped = false;
    let mut expected_writer_epoch = None;
    let mut expected_query_route = None;
    let mut expected_query_route_generation = None;
    let mut expected_holder_token = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--apply" => {
                apply = true;
                index += 1;
            }
            "--confirm-app-stopped" => {
                confirmed_stopped = true;
                index += 1;
            }
            "--expected-writer-epoch" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                expected_writer_epoch = Some(value.parse::<i64>()?);
                index += 2;
            }
            "--expected-query-route" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                expected_query_route = Some(
                    HighSearchQueryRoute::from_persisted_str(value)
                        .ok_or("expected query route must be `legacy` or `protected`")?,
                );
                index += 2;
            }
            "--expected-query-route-generation" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                let generation = value.parse::<i64>()?;
                if generation < 0 {
                    return Err("expected query route generation must be non-negative".into());
                }
                expected_query_route_generation = Some(generation);
                index += 2;
            }
            "--expected-holder-token" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if value.trim().is_empty() {
                    return Err("expected holder token must not be empty".into());
                }
                expected_holder_token = Some(value.clone());
                index += 2;
            }
            _ => return Err(USAGE.into()),
        }
    }

    if !apply || !confirmed_stopped {
        return Err(
            "recovery requires both --apply and --confirm-app-stopped; inspect state first".into(),
        );
    }

    let expected_writer_epoch =
        expected_writer_epoch.ok_or("--expected-writer-epoch is required for recovery")?;
    let expected_query_route =
        expected_query_route.ok_or("--expected-query-route is required for recovery")?;
    let expected_query_route_generation = expected_query_route_generation
        .ok_or("--expected-query-route-generation is required for recovery")?;

    Ok(Command::Apply {
        expected_writer_epoch,
        expected_query_route,
        expected_query_route_generation,
        expected_holder_token,
    })
}

fn required_env(name: &'static str) -> Result<String, Box<dyn std::error::Error>> {
    let value = env::var(name)?;
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty").into());
    }
    Ok(value)
}

fn print_status(
    label: &str,
    status: &memo_app_backend::infrastructure::high_search_maintenance_mongodb::HighSearchMaintenanceStatus,
) {
    println!("{label}.mode={}", status.mode());
    println!("{label}.writer_epoch={}", status.writer_epoch());
    println!("{label}.query_route={}", status.query_route());
    println!(
        "{label}.query_route_generation={}",
        status.query_route_generation()
    );
    println!(
        "{label}.active_writer_leases={}",
        status.active_writer_leases()
    );
    println!(
        "{label}.active_query_leases={}",
        status.active_query_leases()
    );
    println!(
        "{label}.holder_token={}",
        status.holder_token().unwrap_or("<none>")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_the_non_destructive_default() {
        assert_eq!(parse_command(vec![]).unwrap(), Command::Status);
        assert_eq!(
            parse_command(vec!["--status".to_string()]).unwrap(),
            Command::Status
        );
    }

    #[test]
    fn recovery_requires_apply_stop_confirmation_and_exact_route_snapshot() {
        assert!(parse_command(vec!["--apply".to_string()]).is_err());
        assert!(parse_command(vec![
            "--apply".to_string(),
            "--confirm-app-stopped".to_string(),
        ])
        .is_err());
        assert!(parse_command(vec![
            "--apply".to_string(),
            "--confirm-app-stopped".to_string(),
            "--expected-writer-epoch".to_string(),
            "7".to_string(),
        ])
        .is_err());
        assert!(parse_command(vec![
            "--apply".to_string(),
            "--confirm-app-stopped".to_string(),
            "--expected-writer-epoch".to_string(),
            "7".to_string(),
            "--expected-query-route".to_string(),
            "invalid".to_string(),
            "--expected-query-route-generation".to_string(),
            "0".to_string(),
        ])
        .is_err());

        assert_eq!(
            parse_command(vec![
                "--apply".to_string(),
                "--confirm-app-stopped".to_string(),
                "--expected-writer-epoch".to_string(),
                "7".to_string(),
                "--expected-query-route".to_string(),
                "protected".to_string(),
                "--expected-query-route-generation".to_string(),
                "3".to_string(),
                "--expected-holder-token".to_string(),
                "holder".to_string(),
            ])
            .unwrap(),
            Command::Apply {
                expected_writer_epoch: 7,
                expected_query_route: HighSearchQueryRoute::Protected,
                expected_query_route_generation: 3,
                expected_holder_token: Some("holder".to_string()),
            }
        );
    }
}
