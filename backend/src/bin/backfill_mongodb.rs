use std::{
    env,
    process::ExitCode,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use memo_app_backend::{
    config::{AppConfig, AuthoritativeBackend},
    infrastructure::persistence::{
        mongodb::{MigrationImportResult, MongoDbAuthoritativeStore},
        scylla::ScyllaDB,
    },
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Scylla -> MongoDB backfill failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let apply = parse_apply_flag()?;
    let scylla_uri = required_env("SCYLLA_URI")?;
    let source = ScyllaDB::connect_existing(&scylla_uri).await?;

    if !apply {
        let count = source.for_each_memo(|_| async { Ok(()) }).await?;
        println!(
            "Dry run complete: found {count} memo(s) in ScyllaDB. Re-run with --apply and explicit MONGODB_URI/MONGODB_DATABASE to copy them."
        );
        return Ok(());
    }

    required_env("MONGODB_URI")?;
    required_env("MONGODB_DATABASE")?;

    let mut vars: Vec<(String, String)> = env::vars()
        .filter(|(name, _)| name != "AUTH_MODE" && name != "AUTHORITATIVE_BACKEND")
        .collect();
    vars.push(("AUTH_MODE".to_string(), "development".to_string()));
    vars.push(("AUTHORITATIVE_BACKEND".to_string(), "mongodb".to_string()));

    let config = AppConfig::from_vars(vars)?;
    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err("migration destination did not resolve to MongoDB".into());
    }

    let destination =
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?;
    let inserted = Arc::new(AtomicUsize::new(0));
    let already_present = Arc::new(AtomicUsize::new(0));

    let visited = source
        .for_each_memo(|memo| {
            let inserted = Arc::clone(&inserted);
            let already_present = Arc::clone(&already_present);
            let destination = &destination;

            async move {
                match destination.import_memo_for_migration(&memo).await? {
                    MigrationImportResult::Inserted => {
                        inserted.fetch_add(1, Ordering::Relaxed);
                    }
                    MigrationImportResult::AlreadyPresent => {
                        already_present.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Ok(())
            }
        })
        .await?;

    let inserted = inserted.load(Ordering::Relaxed);
    let already_present = already_present.load(Ordering::Relaxed);

    println!(
        "Backfill complete: visited={visited} inserted={inserted} already_present={already_present}"
    );
    println!(
        "Do not switch AUTHORITATIVE_BACKEND yet. Rebuild or isolate Valkey/Manticore projections, verify application reads, then perform the documented cutover."
    );

    Ok(())
}

fn parse_apply_flag() -> Result<bool, Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [] => Ok(false),
        [flag] if flag == "--apply" => Ok(true),
        _ => Err("usage: backfill_mongodb [--apply]".into()),
    }
}

fn required_env(name: &'static str) -> Result<String, Box<dyn std::error::Error>> {
    let value = env::var(name)?;
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty").into());
    }
    Ok(value)
}
