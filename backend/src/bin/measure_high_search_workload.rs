use std::{env, fs, path::PathBuf, process::ExitCode};

use memo_app_backend::infrastructure::high_search_workload_measurement::{
    measure_high_search_workload_json, MAX_HIGH_SEARCH_WORKLOAD_FILE_BYTES,
};

const USAGE: &str =
    "usage: measure_high_search_workload --input <sanitized-or-generated-corpus.json>";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("HIGH search workload measurement failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let input_path = parse_args(env::args().skip(1).collect())?;
    let metadata = fs::metadata(&input_path)
        .map_err(|error| format!("failed to inspect workload input file: {error}"))?;
    if metadata.len() > MAX_HIGH_SEARCH_WORKLOAD_FILE_BYTES {
        return Err(format!(
            "workload input exceeds the {} byte safety limit",
            MAX_HIGH_SEARCH_WORKLOAD_FILE_BYTES
        )
        .into());
    }

    let input = fs::read_to_string(&input_path)
        .map_err(|error| format!("failed to read workload input file as UTF-8: {error}"))?;
    let report = measure_high_search_workload_json(&input)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    match args.as_slice() {
        [flag, path] if flag == "--input" && !path.trim().is_empty() => Ok(PathBuf::from(path)),
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_path_is_explicit_and_single_purpose() {
        assert_eq!(
            parse_args(vec!["--input".into(), "corpus.json".into()]).unwrap(),
            PathBuf::from("corpus.json")
        );
        assert!(parse_args(Vec::new()).is_err());
        assert!(parse_args(vec!["--input".into(), "".into()]).is_err());
        assert!(parse_args(vec!["--other".into(), "corpus.json".into()]).is_err());
    }
}
