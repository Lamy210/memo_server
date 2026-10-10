use std::{fs, path::Path};

fn read_source(relative_path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

#[test]
fn production_memo_traits_do_not_expose_unbounded_owner_scan() {
    for relative_path in [
        "src/domain/memo/repository.rs",
        "src/infrastructure/persistence/ports.rs",
    ] {
        let source = read_source(relative_path);

        assert!(
            !source.contains("async fn find_all_by_user_id("),
            "{relative_path} must not expose an unbounded owner-scan API; use list_page_by_user_id"
        );
        assert!(
            source.contains("async fn list_page_by_user_id("),
            "{relative_path} must expose bounded cursor pagination as the memo-list contract"
        );
    }
}

#[test]
fn production_memo_implementations_do_not_restore_unbounded_owner_scan() {
    for relative_path in [
        "src/infrastructure/repositories/memo.rs",
        "src/infrastructure/high_memo_authoritative.rs",
        "src/infrastructure/persistence/mongodb.rs",
        "src/infrastructure/persistence/scylla.rs",
    ] {
        let source = read_source(relative_path);

        assert!(
            !source.contains("async fn find_all_by_user_id("),
            "{relative_path} must not implement an unbounded owner-scan API"
        );
    }
}

#[test]
fn memo_service_never_calls_unbounded_owner_scan() {
    let service = read_source("src/application/memo/service.rs");

    assert!(
        !service.contains(".find_all_by_user_id("),
        "MemoService request paths must use list_page_by_user_id instead of an unbounded owner scan"
    );
}
