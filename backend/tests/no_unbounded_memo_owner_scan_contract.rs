use std::{fs, path::Path};

const FORBIDDEN_SYMBOLS: [&str; 2] = ["find_all_by_user_id", "find_all_envelopes_by_owner"];
const PRODUCTION_MEMO_MODULES: [&str; 7] = [
    "src/domain/memo/repository.rs",
    "src/infrastructure/repositories/memo.rs",
    "src/infrastructure/persistence/ports.rs",
    "src/infrastructure/persistence/scylla.rs",
    "src/infrastructure/persistence/mongodb.rs",
    "src/infrastructure/high_memo_authoritative.rs",
    "src/application/memo/service.rs",
];

#[test]
fn production_memo_modules_do_not_expose_unbounded_owner_scans() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();

    for relative_path in PRODUCTION_MEMO_MODULES {
        let path = manifest_dir.join(relative_path);
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

        for symbol in FORBIDDEN_SYMBOLS {
            if source.contains(symbol) {
                violations.push(format!("{relative_path}: {symbol}"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "unbounded memo owner-scan APIs must not exist in production modules:\n{}",
        violations.join("\n")
    );
}
