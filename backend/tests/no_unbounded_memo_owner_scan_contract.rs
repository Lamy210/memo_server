use std::{fs, path::Path};

fn read_source(relative_path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

#[test]
fn production_repository_contracts_do_not_expose_unbounded_owner_scans() {
    for relative_path in [
        "src/domain/memo/repository.rs",
        "src/infrastructure/persistence/ports.rs",
    ] {
        let source = read_source(relative_path);
        assert!(
            !source.contains("find_all_by_user_id"),
            "{relative_path} must not expose the legacy unbounded owner-scan API"
        );
        assert!(
            source.contains("list_page_by_user_id"),
            "{relative_path} must expose bounded cursor pagination as the supported list contract"
        );
    }

    let ports = read_source("src/infrastructure/persistence/ports.rs");
    assert!(
        !ports.contains("find_all_envelopes_by_owner"),
        "encrypted authoritative store contract must not expose an all-owner envelope loader"
    );
    assert!(
        ports.contains("page_envelopes_by_owner"),
        "encrypted authoritative store contract must expose bounded owner pagination"
    );
}

#[test]
fn memo_service_never_calls_unbounded_owner_scan() {
    let service = read_source("src/application/memo/service.rs");

    assert!(
        !service.contains(".find_all_by_user_id("),
        "MemoService request paths must use list_page_by_user_id instead of an unbounded owner scan"
    );
}
