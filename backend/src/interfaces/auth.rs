use actix_web::{
    dev::Payload,
    http::header::AUTHORIZATION,
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
        let authorization = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let development_user_id = request
            .headers()
            .get(DEVELOPMENT_USER_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        Box::pin(async move {
            let auth_service = auth_service.ok_or_else(|| {
                AppError::InternalServerError("Authentication service is not configured".into())
            })?;
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
