use actix_web::{
    dev::Payload,
    http::header::{HeaderValue, AUTHORIZATION},
    web::Data,
    FromRequest, HttpRequest,
};
use futures::future::LocalBoxFuture;

use crate::{
    error::{AppError, AppResult},
    infrastructure::{
        auth::{AuthService, AuthenticatedIdentity},
        auth_observability::observe_authentication_result,
    },
};

const DEVELOPMENT_USER_HEADER: &str = "x-development-user-id";

pub struct AuthenticatedUser(pub AuthenticatedIdentity);

impl FromRequest for AuthenticatedUser {
    type Error = AppError;
    type Future = LocalBoxFuture<'static, AppResult<Self>>;

    fn from_request(request: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let auth_service = request.app_data::<Data<AuthService>>().cloned();
        let authorization =
            single_header_value(request.headers().get_all(AUTHORIZATION), "Authorization");
        let development_user_id = single_header_value(
            request.headers().get_all(DEVELOPMENT_USER_HEADER),
            "X-Development-User-Id",
        );

        Box::pin(async move {
            let result = async {
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
            }
            .await;

            observe_authentication_result(result)
        })
    }
}

fn single_header_value<'a>(
    mut values: impl Iterator<Item = &'a HeaderValue>,
    header_name: &str,
) -> AppResult<Option<String>> {
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(AppError::Unauthorized(format!(
            "{header_name} header must not be repeated"
        )));
    }

    let value = value.to_str().map_err(|_| {
        AppError::Unauthorized(format!("{header_name} header contains invalid bytes"))
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
    use crate::infrastructure::auth_observability::{
        classify_authentication_result, AuthObservability, AuthVerificationOutcome,
        AuthVerificationStats,
    };

    #[test]
    fn single_authentication_header_is_accepted() {
        let values = [HeaderValue::from_static("Bearer aaa.bbb.ccc")];
        assert_eq!(
            single_header_value(values.iter(), "Authorization").unwrap(),
            Some("Bearer aaa.bbb.ccc".to_string())
        );
    }

    #[test]
    fn duplicate_authentication_headers_are_rejected() {
        let values = [
            HeaderValue::from_static("Bearer aaa.bbb.ccc"),
            HeaderValue::from_static("Bearer ddd.eee.fff"),
        ];
        assert!(matches!(
            single_header_value(values.iter(), "Authorization"),
            Err(AppError::Unauthorized(_))
        ));
    }

    #[test]
    fn invalid_header_bytes_are_rejected_instead_of_treated_as_missing() {
        let values = [HeaderValue::from_bytes(&[0xFF]).unwrap()];
        assert!(matches!(
            single_header_value(values.iter(), "Authorization"),
            Err(AppError::Unauthorized(_))
        ));
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

    #[test]
    fn authentication_observability_uses_fixed_secret_free_categories() {
        let success: AppResult<()> = Ok(());
        assert_eq!(
            classify_authentication_result(&success),
            AuthVerificationOutcome::Success
        );

        let secret = "sensitive-token-shaped-detail";
        let unauthorized: AppResult<()> = Err(AppError::Unauthorized(secret.into()));
        let unavailable: AppResult<()> = Err(AppError::ServiceUnavailable(secret.into()));
        let internal: AppResult<()> = Err(AppError::InternalServerError(secret.into()));

        let cases = [
            (
                classify_authentication_result(&unauthorized),
                AuthVerificationOutcome::Unauthorized,
                "unauthorized",
            ),
            (
                classify_authentication_result(&unavailable),
                AuthVerificationOutcome::DependencyUnavailable,
                "dependency_unavailable",
            ),
            (
                classify_authentication_result(&internal),
                AuthVerificationOutcome::InternalError,
                "internal_error",
            ),
        ];

        for (actual, expected, label) in cases {
            assert_eq!(actual, expected);
            assert_eq!(actual.label(), label);
            assert!(!actual.label().contains(secret));
        }
    }

    #[test]
    fn authentication_observability_aggregates_fixed_outcomes() {
        let observability = AuthObservability::new();
        let secret = "sensitive-token-shaped-detail";

        let success: AppResult<()> = Ok(());
        let unauthorized: AppResult<()> = Err(AppError::Unauthorized(secret.into()));
        let unavailable: AppResult<()> = Err(AppError::ServiceUnavailable(secret.into()));
        let internal: AppResult<()> = Err(AppError::InternalServerError(secret.into()));

        observability.record(&success);
        observability.record(&unauthorized);
        observability.record(&unavailable);
        observability.record(&internal);

        assert_eq!(
            observability.snapshot(),
            AuthVerificationStats {
                total: 4,
                success: 1,
                unauthorized: 1,
                dependency_unavailable: 1,
                internal_error: 1,
            }
        );
    }

    #[test]
    fn authentication_observability_preserves_the_original_result() {
        let observability = AuthObservability::new();
        let secret = "sensitive-token-shaped-detail";
        let result: AppResult<u8> = Err(AppError::Unauthorized(secret.into()));

        let returned = observability.record_and_return(result);

        assert!(matches!(
            &returned,
            Err(AppError::Unauthorized(message)) if message == secret
        ));
        assert_eq!(
            observability.snapshot(),
            AuthVerificationStats {
                total: 1,
                success: 0,
                unauthorized: 1,
                dependency_unavailable: 0,
                internal_error: 0,
            }
        );
    }
}
