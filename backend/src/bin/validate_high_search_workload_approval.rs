use std::{env, fs, path::PathBuf, process::ExitCode};

use memo_app_backend::{
    config::AppConfig,
    infrastructure::high_search_workload_approval::{
        parse_high_search_workload_approval_json, MAX_HIGH_SEARCH_WORKLOAD_APPROVAL_FILE_BYTES,
    },
};

const USAGE: &str =
    "usage: validate_high_search_workload_approval --input <approval.json> [--against-env]";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    input: PathBuf,
    against_env: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("HIGH search workload approval validation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(env::args().skip(1).collect())?;
    let metadata = fs::metadata(&args.input)
        .map_err(|error| format!("failed to inspect workload approval file: {error}"))?;
    if metadata.len() > MAX_HIGH_SEARCH_WORKLOAD_APPROVAL_FILE_BYTES {
        return Err(format!(
            "workload approval exceeds the {} byte safety limit",
            MAX_HIGH_SEARCH_WORKLOAD_APPROVAL_FILE_BYTES
        )
        .into());
    }

    let input = fs::read_to_string(&args.input)
        .map_err(|error| format!("failed to read workload approval file as UTF-8: {error}"))?;
    let approval = parse_high_search_workload_approval_json(&input)?;

    if args.against_env {
        let config = AppConfig::from_env()?;
        approval.validate_against_config(&config.high_search)?;
    }

    println!("approval.valid=true");
    println!("approval.id={}", approval.approval_id);
    println!(
        "approval.analysis_version={}",
        approval.measurement.analysis_version
    );
    println!("approval.documents={}", approval.measurement.documents);
    println!("approval.queries={}", approval.measurement.queries);
    println!(
        "approval.max_document_content_terms={}",
        approval.selected_budgets.max_document_content_terms
    );
    println!(
        "approval.max_query_content_terms={}",
        approval.selected_budgets.max_query_content_terms
    );
    println!(
        "approval.max_normalized_term_bytes={}",
        approval.selected_budgets.max_normalized_term_bytes
    );
    println!("approval.against_env={}", args.against_env);
    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<Args, Box<dyn std::error::Error>> {
    let mut input = None;
    let mut against_env = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--input" => {
                let value = args.get(index + 1).ok_or(USAGE)?;
                if value.trim().is_empty() || input.is_some() {
                    return Err(USAGE.into());
                }
                input = Some(PathBuf::from(value));
                index += 2;
            }
            "--against-env" => {
                if against_env {
                    return Err(USAGE.into());
                }
                against_env = true;
                index += 1;
            }
            _ => return Err(USAGE.into()),
        }
    }

    let input = input.ok_or(USAGE)?;
    Ok(Args { input, against_env })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_input_is_explicit_and_env_check_is_opt_in() {
        assert_eq!(
            parse_args(vec!["--input".into(), "approval.json".into()]).unwrap(),
            Args {
                input: PathBuf::from("approval.json"),
                against_env: false,
            }
        );
        assert_eq!(
            parse_args(vec![
                "--against-env".into(),
                "--input".into(),
                "approval.json".into(),
            ])
            .unwrap(),
            Args {
                input: PathBuf::from("approval.json"),
                against_env: true,
            }
        );

        assert!(parse_args(Vec::new()).is_err());
        assert!(parse_args(vec!["--input".into(), "".into()]).is_err());
        assert!(parse_args(vec!["--against-env".into()]).is_err());
        assert!(parse_args(vec![
            "--input".into(),
            "a.json".into(),
            "--input".into(),
            "b.json".into(),
        ])
        .is_err());
    }
}
