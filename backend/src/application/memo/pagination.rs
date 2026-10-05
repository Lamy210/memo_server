#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        format_cursor_v1, parse_cursor_v1, resolve_memo_list_limit, MEMO_LIST_DEFAULT_LIMIT,
        MEMO_LIST_MAX_LIMIT,
    };
    use crate::error::AppError;

    const V4_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn cursor_v1_accepts_canonical_lowercase_uuid_v4() {
        let expected = Uuid::parse_str(V4_ID).unwrap();

        assert_eq!(parse_cursor_v1(&format!("v1.{V4_ID}")).unwrap(), expected);
        assert_eq!(format_cursor_v1(expected), format!("v1.{V4_ID}"));
    }

    #[test]
    fn cursor_v1_rejects_noncanonical_or_unsupported_values() {
        for value in [
            "550e8400-e29b-41d4-a716-446655440000",
            "v2.550e8400-e29b-41d4-a716-446655440000",
            "v1.550E8400-E29B-41D4-A716-446655440000",
            "v1.550e8400-e29b-11d4-a716-446655440000",
            "v1.00000000-0000-0000-0000-000000000000",
            "v1.550e8400-e29b-41d4-a716-446655440000.trailing",
            "v1.550e8400-e29b-41d4-a716-446655440000\n",
        ] {
            assert!(
                matches!(parse_cursor_v1(value), Err(AppError::BadRequest(_))),
                "{value}"
            );
        }
    }

    #[test]
    fn memo_list_limit_defaults_to_twenty_and_accepts_explicit_bounds() {
        assert_eq!(MEMO_LIST_DEFAULT_LIMIT, 20);
        assert_eq!(MEMO_LIST_MAX_LIMIT, 100);
        assert_eq!(resolve_memo_list_limit(None).unwrap(), 20);
        assert_eq!(resolve_memo_list_limit(Some(1)).unwrap(), 1);
        assert_eq!(resolve_memo_list_limit(Some(100)).unwrap(), 100);
    }

    #[test]
    fn memo_list_limit_rejects_values_outside_the_contract() {
        assert!(matches!(
            resolve_memo_list_limit(Some(0)),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            resolve_memo_list_limit(Some(101)),
            Err(AppError::BadRequest(_))
        ));
    }
}
