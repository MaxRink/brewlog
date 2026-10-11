use askama::Template;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_cookies::{Cookie, Cookies};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::application::auth::{AuthenticatedUser, SESSION_COOKIE_NAME};
use crate::application::routes::render_html;
use crate::application::state::{AppState, PendingOidc};
use crate::domain::RepositoryError;
use crate::domain::users::{NewUser, is_valid_username};
use crate::infrastructure::auth::generate_session_token;
use crate::infrastructure::auth::hash_token;
use crate::infrastructure::oidc::OidcIdentity;

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    pub cli_callback: Option<String>,
}

#[derive(Template)]
#[template(path = "pages/login.html")]
struct LoginTemplate {
    nav_active: &'static str,
    is_authenticated: bool,
    version_info: &'static crate::VersionInfo,
    oidc_enabled: bool,
}

#[tracing::instrument(skip(state, cookies))]
pub(crate) async fn login_page(
    State(state): State<AppState>,
    cookies: Cookies,
    Query(query): Query<LoginQuery>,
) -> Result<Response, StatusCode> {
    // Don't redirect when CLI callback params are present — the user needs
    // to authenticate with their passkey to generate a bearer token for the CLI.
    if query.cli_callback.is_none() && is_authenticated(&state, &cookies).await {
        return Ok(Redirect::to("/").into_response());
    }

    let template = LoginTemplate {
        nav_active: "login",
        is_authenticated: false,
        version_info: &crate::VERSION_INFO,
        oidc_enabled: state.oidc.is_some(),
    };

    render_html(template).map(IntoResponse::into_response)
}

const OIDC_STATE_COOKIE: &str = "brewlog_oidc_state";

#[derive(Debug, Deserialize)]
pub struct OidcCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

pub(crate) async fn oidc_login(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<Redirect, StatusCode> {
    start_oidc(&state, &cookies, None).await
}

/// Start an explicit, session-bound link for the currently authenticated
/// local account. The callback can only link the identity to this user; OIDC
/// email claims are never used to select or take over an account.
pub(crate) async fn oidc_link(
    State(state): State<AppState>,
    AuthenticatedUser(user): AuthenticatedUser,
    cookies: Cookies,
) -> Result<Redirect, StatusCode> {
    start_oidc(&state, &cookies, Some(user.id)).await
}

async fn start_oidc(
    state: &AppState,
    cookies: &Cookies,
    link_user_id: Option<crate::domain::ids::UserId>,
) -> Result<Redirect, StatusCode> {
    let Some(service) = &state.oidc else {
        return Err(StatusCode::NOT_FOUND);
    };
    let authorization = service.authorization();
    let state_value = authorization.state.secret().to_string();
    {
        let mut pending = state.oidc_pending.lock().await;
        pending.retain(|_, value| value.created_at.elapsed() < std::time::Duration::from_secs(600));
        pending.insert(
            state_value.clone(),
            PendingOidc {
                nonce: authorization.nonce,
                verifier: authorization.verifier,
                created_at: std::time::Instant::now(),
                link_user_id,
            },
        );
    }
    let mut cookie = Cookie::new(OIDC_STATE_COOKIE, state_value);
    cookie.set_path("/auth/oidc");
    cookie.set_http_only(true);
    cookie.set_same_site(tower_cookies::cookie::SameSite::Lax);
    cookie.set_max_age(tower_cookies::cookie::time::Duration::minutes(10));
    if !state.insecure_cookies {
        cookie.set_secure(true);
    }
    cookies.add(cookie);
    Ok(Redirect::to(authorization.url.as_str()))
}

pub(crate) async fn oidc_callback(
    State(state): State<AppState>,
    cookies: Cookies,
    Query(query): Query<OidcCallbackQuery>,
) -> Result<Redirect, StatusCode> {
    if query.error.is_some() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let state_value = query.state.ok_or(StatusCode::BAD_REQUEST)?;
    let cookie_state = cookies
        .get(OIDC_STATE_COOKIE)
        .ok_or(StatusCode::BAD_REQUEST)?;
    if cookie_state.value() != state_value {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut expired_state = Cookie::from(OIDC_STATE_COOKIE);
    expired_state.set_path("/auth/oidc");
    cookies.remove(expired_state);
    let pending = state
        .oidc_pending
        .lock()
        .await
        .remove(&state_value)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if pending.created_at.elapsed() >= std::time::Duration::from_secs(600) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let code = query.code.ok_or(StatusCode::BAD_REQUEST)?;
    let service = state.oidc.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let identity = service
        .exchange(
            openidconnect::AuthorizationCode::new(code),
            pending.verifier,
            pending.nonce,
        )
        .await
        .map_err(|err| {
            warn!(error = %err, "OIDC login failed");
            StatusCode::UNAUTHORIZED
        })?;
    let identity_key = service.identity_key(&identity.subject);
    let linked = state
        .user_repo
        .get_by_oidc_identity(service.issuer(), &identity.subject)
        .await;
    let user = if let Some(target_id) = pending.link_user_id {
        match linked {
            Ok(existing) if existing.id == target_id => existing,
            Ok(_) => return Err(StatusCode::CONFLICT),
            Err(RepositoryError::NotFound) => {
                state
                    .user_repo
                    .link_oidc_identity(target_id, service.issuer(), &identity.subject)
                    .await
                    .map_err(|err| {
                        error!(error = %err, "failed to link OIDC identity");
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;
                state.user_repo.get(target_id).await.map_err(|err| {
                    error!(error = %err, "failed to reload linked OIDC user");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
            }
            Err(err) => {
                error!(error = %err, "OIDC identity lookup failed");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    } else {
        match linked {
            Ok(user) => user,
            Err(RepositoryError::NotFound) => {
                provision_oidc_user(&state, &identity, &identity_key).await?
            }
            Err(err) => {
                error!(error = %err, "OIDC identity lookup failed");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    };
    create_session_for_user(&state, &cookies, user.id).await?;
    Ok(Redirect::to("/"))
}

async fn provision_oidc_user(
    state: &AppState,
    identity: &OidcIdentity,
    identity_key: &str,
) -> Result<crate::domain::users::User, StatusCode> {
    let service = state.oidc.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let username = oidc_username(identity);
    let existing = state.user_repo.get_by_username(&username).await;
    let username = if existing.is_ok() {
        format!("{}-{}", username, identity_key_suffix(identity_key))
    } else {
        username
    };
    let new_user = NewUser::new(username, Uuid::new_v4().to_string());
    if !is_valid_username(&new_user.username) {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let user = state.user_repo.insert(new_user).await.map_err(|err| {
        error!(error = %err, "failed to provision OIDC user");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    state
        .user_repo
        .link_oidc_identity(user.id, service.issuer(), &identity.subject)
        .await
        .map_err(|err| {
            error!(error = %err, "failed to link OIDC identity");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(user)
}

fn oidc_username(identity: &OidcIdentity) -> String {
    let source = identity
        .email
        .as_deref()
        .and_then(|email| email.split('@').next())
        .unwrap_or("oidc-user");
    let candidate: String = source
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24)
        .collect();
    if is_valid_username(&candidate) {
        candidate
    } else {
        "oidc-user".to_string()
    }
}

fn identity_key_suffix(identity_key: &str) -> String {
    hash_token(identity_key)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect()
}

async fn create_session_for_user(
    state: &AppState,
    cookies: &Cookies,
    user_id: crate::domain::ids::UserId,
) -> Result<(), StatusCode> {
    let session_token = generate_session_token();
    let session_token_hash = hash_token(&session_token);
    let now = chrono::Utc::now();
    state
        .session_repo
        .insert(crate::domain::sessions::NewSession::new(
            user_id,
            session_token_hash,
            now,
            now + chrono::Duration::days(30),
        ))
        .await
        .map_err(|err| {
            error!(error = %err, "failed to create OIDC session");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let mut cookie = Cookie::new(SESSION_COOKIE_NAME, session_token);
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(tower_cookies::cookie::SameSite::Lax);
    cookie.set_max_age(tower_cookies::cookie::time::Duration::days(30));
    if !state.insecure_cookies {
        cookie.set_secure(true);
    }
    cookies.add(cookie);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oidc_username_does_not_accept_email_punctuation() {
        let identity = OidcIdentity {
            subject: "subject".to_string(),
            email: Some("max+coffee@example.test".to_string()),
            username: None,
        };
        assert_eq!(oidc_username(&identity), "maxcoffee");
    }

    #[test]
    fn invalid_or_missing_email_uses_safe_fallback() {
        let identity = OidcIdentity {
            subject: "subject".to_string(),
            email: None,
            username: None,
        };
        assert_eq!(oidc_username(&identity), "oidc-user");
    }
}

#[tracing::instrument(skip(state, cookies))]
pub(crate) async fn logout(State(state): State<AppState>, cookies: Cookies) -> Redirect {
    if let Some(cookie) = cookies.get(SESSION_COOKIE_NAME) {
        let session_token = cookie.value();
        let session_token_hash = hash_token(session_token);

        if let Ok(session) = state
            .session_repo
            .get_by_token_hash(&session_token_hash)
            .await
            && let Err(err) = state.session_repo.delete(session.id).await
        {
            warn!(error = %err, session_id = %session.id, "failed to delete session on logout");
        }
    }

    info!("user logged out");
    cookies.remove(Cookie::from(SESSION_COOKIE_NAME));
    Redirect::to("/")
}

#[tracing::instrument(skip(state, cookies))]
pub async fn is_authenticated(state: &AppState, cookies: &Cookies) -> bool {
    let Some(cookie) = cookies.get(SESSION_COOKIE_NAME) else {
        return false;
    };

    let session_token = cookie.value();
    let session_token_hash = hash_token(session_token);

    match state
        .session_repo
        .get_by_token_hash(&session_token_hash)
        .await
    {
        Ok(session) => !session.is_expired(),
        Err(err) => {
            warn!(error = %err, "session lookup failed during auth check");
            false
        }
    }
}
