use std::{fs, path::Path};

const DEPRECATED_OWNER_SCAN: &str = "#[deprecated(note = \"unbounded owner scans are forbidden on request paths; use list_page_by_user_id\")]\n    async fn find_all_by_user_id";

fn read_source(relative_path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

#[test]
fn application_repository_marks_unbounded_owner_scan_as_deprecated() {
    let repository = read_source("src/domain/memo/repository.rs");

    assert!(
        repository.contains(DEPRECATED_OWNER_SCAN),
        "MemoRepository::find_all_by_user_id must remain explicitly deprecated while compatibility keeps it in the trait"
    );
    assert!(
        repository.contains("async fn list_page_by_user_id("),
        "MemoRepository must expose bounded cursor pagination as the supported list contract"
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
