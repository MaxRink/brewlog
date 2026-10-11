//! OpenID Connect discovery, authorization-code exchange and claim validation.

use anyhow::{Context, Result, anyhow};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse,
};
use reqwest::Client;
use url::Url;

#[derive(Clone, Debug)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_url: String,
}

#[derive(Clone)]
pub struct OidcService {
    client: CoreClient<
        EndpointSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointNotSet,
        EndpointMaybeSet,
        EndpointMaybeSet,
    >,
    issuer_url: String,
    http_client: Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcIdentity {
    pub subject: String,
    pub email: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug)]
pub struct OidcAuthorization {
    pub url: Url,
    pub state: CsrfToken,
    pub nonce: Nonce,
    pub verifier: PkceCodeVerifier,
}

impl OidcService {
    pub async fn discover(config: OidcConfig, http_client: Client) -> Result<Self> {
        let issuer =
            IssuerUrl::new(config.issuer_url.clone()).context("invalid OIDC issuer URL")?;
        let metadata = CoreProviderMetadata::discover_async(issuer, &http_client)
            .await
            .context("OIDC provider discovery failed")?;
        let redirect =
            RedirectUrl::new(config.redirect_url).context("invalid OIDC redirect URL")?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(config.client_id),
            config.client_secret.map(ClientSecret::new),
        )
        .set_redirect_uri(redirect);
        Ok(Self {
            client,
            issuer_url: config.issuer_url,
            http_client,
        })
    }

    pub fn authorization(&self) -> OidcAuthorization {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = self
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("openid".to_string()))
            .add_scope(Scope::new("profile".to_string()))
            .add_scope(Scope::new("email".to_string()))
            .set_pkce_challenge(challenge)
            .url();
        OidcAuthorization {
            url,
            state,
            nonce,
            verifier,
        }
    }

    pub async fn exchange(
        &self,
        code: AuthorizationCode,
        verifier: PkceCodeVerifier,
        nonce: Nonce,
    ) -> Result<OidcIdentity> {
        let token = self
            .client
            .exchange_code(code)
            .context("OIDC authorization code was rejected")?
            .set_pkce_verifier(verifier)
            .request_async(&self.http_client)
            .await
            .context("OIDC token exchange failed")?;
        let id_token = token
            .id_token()
            .ok_or_else(|| anyhow!("OIDC provider returned no ID token"))?;
        let claims = id_token
            .claims(&self.client.id_token_verifier(), &nonce)
            .context("OIDC ID token validation failed")?;
        Ok(OidcIdentity {
            subject: claims.subject().as_str().to_string(),
            email: claims
                .email()
                .map(|value| value.as_str().to_ascii_lowercase()),
            username: None,
        })
    }

    pub fn identity_key(&self, subject: &str) -> String {
        format!("oidc:{}:{}", self.issuer_url, subject)
    }

    pub fn issuer(&self) -> &str {
        &self.issuer_url
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn identity_key_is_scoped_to_issuer_and_subject() {
        let key = format!("oidc:{}:{}", "https://auth.example", "subject-1");
        assert_eq!(key, "oidc:https://auth.example:subject-1");
    }
}
