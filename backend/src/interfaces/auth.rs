use actix_web::{
    dev::Payload,
    http::header::{HeaderValue, AUTHORIZATION},
    web::Data,
    FromRequest, HttpRequest,
};
use futures::future::LocalBoxFuture;

use crate::{
    error::{AppError, AppResult},
    infrastructure::auth::{AuthService, AuthenticatedIdentity},
};

const DEVELOPMENT_USER_HEADER: &str = "x-development-user-id";

pub struct AuthenticatedUser(pub AuthenticatedIdentity);

impl FromRequest for AuthenticatedUser {
    type Error = AppError;
    type Future = LocalBoxFuture<'static, AppResult<Self>>;

    fn from_request(request: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let auth_service = request.app_data::<Data<AuthService>>().cloned();
        let authorization = single_auth_header_value(
            request.headers().get_all(AUTHORIZATION).iter(),
            "Authorization",
        );
        let development_user_id = single_auth_header_value(
            request.headers().get_all(DEVELOPMENT_USER_HEADER).iter(),
            "X-Development-User-Id",
        );

        Box::pin(async move {
            let auth_service = auth_service.ok_or_else(|| {
                AppError::InternalServerError("Authentication service is not configured".into())
            })?;
            let authorization = authorization?;
            let development_user_id = development_user_id?;
            let bearer_token = authorization
                .as_deref()
                .map(parse_bearer_token)
                .transpose()?;

            let identity = auth_service
                .authenticate(bearer_token, development_user_id.as_deref())
                .await?;

            Ok(Self(identity))
        })
    }
}

fn single_auth_header_value<'a>(
    mut values: impl Iterator<Item = &'a HeaderValue>,
    header_name: &'static str,
) -> AppResult<Option<String>> {
    let Some(first) = values.next() else {
        return Ok(None);
    };

    if values.next().is_some() {
        return Err(AppError::Unauthorized(format!(
            "{header_name} header must not be repeated"
        )));
    }

    let value = first.to_str().map_err(|_| {
        AppError::Unauthorized(format!(
            "{header_name} header contains unsupported bytes"
        ))
    })?;
    Ok(Some(value.to_owned()))
}

fn parse_bearer_token(value: &str) -> AppResult<&str> {
    let (scheme, token) = value.split_once(' ').ok_or_else(|| {
        AppError::Unauthorized("Authorization header must use Bearer authentication".into())
    })?;

    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.contains(char::is_whitespace)
    {
        return Err(AppError::Unauthorized(
            "Authorization header must contain one Bearer token".into(),
        ));
    }

    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_header_extraction_rejects_duplicate_or_non_utf8_values() {
        let mut headers = actix_web::http::header::HeaderMap::new();
        headers.append(AUTHORIZATION, HeaderValue::from_static("Bearer first"));
        headers.append(AUTHORIZATION, HeaderValue::from_static("Bearer second"));
        assert!(matches!(
            single_auth_header_value(headers.get_all(AUTHORIZATION).iter(), "Authorization"),
            Err(AppError::Unauthorized(_))
        ));

        let mut headers = actix_web::http::header::HeaderMap::new();
        headers.append(
            DEVELOPMENT_USER_HEADER,
            HeaderValue::from_static("12345678-1234-4234-8234-123456789012"),
        );
        headers.append(
            DEVELOPMENT_USER_HEADER,
            HeaderValue::from_static("87654321-4321-4321-8321-210987654321"),
        );
        assert!(matches!(
            single_auth_header_value(
                headers.get_all(DEVELOPMENT_USER_HEADER).iter(),
                "X-Development-User-Id",
            ),
            Err(AppError::Unauthorized(_))
        ));

        let mut headers = actix_web::http::header::HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_bytes(&[0xFF]).expect("opaque header bytes are structurally valid"),
        );
        assert!(matches!(
            single_auth_header_value(headers.get_all(AUTHORIZATION).iter(), "Authorization"),
            Err(AppError::Unauthorized(_))
        ));
    }

    #[test]
    fn auth_header_extraction_preserves_single_value_and_absence() {
        let mut headers = actix_web::http::header::HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer aaa.bbb.ccc"));

        assert_eq!(
            single_auth_header_value(headers.get_all(AUTHORIZATION).iter(), "Authorization")
                .unwrap(),
            Some("Bearer aaa.bbb.ccc".to_string())
        );

        let empty = actix_web::http::header::HeaderMap::new();
        assert_eq!(
            single_auth_header_value(empty.get_all(AUTHORIZATION).iter(), "Authorization")
                .unwrap(),
            None
        );
    }

    #[test]
    fn bearer_parser_accepts_case_insensitive_scheme() {
        assert_eq!(
            parse_bearer_token("bEaReR aaa.bbb.ccc").expect("valid bearer token"),
            "aaa.bbb.ccc"
        );
    }

    #[test]
    fn bearer_parser_rejects_multiple_token_parts() {
        assert!(matches!(
            parse_bearer_token("Bearer one two"),
            Err(AppError::Unauthorized(_))
        ));
    }
}
