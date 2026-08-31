use async_trait::async_trait;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT},
    Client, Response, StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

use crate::{DestinationRepository, RepositoryAccess, RepositoryGrant};

/// REST API version sent by the simulator GitHub client.
pub const GITHUB_API_VERSION: &str = "2026-03-10";
const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;

/// Repository facts returned by GitHub after creating a session repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedRepository {
    /// Numeric GitHub repository identifier.
    pub id: u64,
    /// Repository name without its owner.
    pub name: String,
    /// Uncredentialed HTTPS clone URL.
    pub clone_url: String,
    /// Whether the repository is private.
    pub private: bool,
    /// Default branch configured by GitHub or the organization.
    pub default_branch: String,
}

/// Result of granting one actor access to one session repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantOutcome {
    /// GitHub created an invitation that the user still has to accept.
    InvitationCreated,
    /// GitHub applied access immediately or updated an existing collaborator.
    AccessUpdated,
}

/// Failures returned by the GitHub repository transport.
#[derive(Debug, thiserror::Error)]
pub enum GitHubApiError {
    /// The API base URL was malformed or used insecure HTTP away from loopback.
    #[error("invalid GitHub API base URL: {0}")]
    InvalidBaseUrl(String),
    /// The installation token could not be represented as an HTTP header.
    #[error("invalid GitHub installation token")]
    InvalidToken,
    /// GitHub returned a non-success status code.
    #[error("{operation} returned HTTP {status}: {body}")]
    HttpStatus {
        /// Stable operation name.
        operation: &'static str,
        /// Numeric HTTP status.
        status: u16,
        /// Bounded response body.
        body: String,
    },
    /// The HTTP request could not be completed.
    #[error("GitHub transport error: {0}")]
    Transport(String),
    /// GitHub returned success with an invalid or inconsistent body.
    #[error("{operation} returned an invalid response: {reason}")]
    InvalidResponse {
        /// Stable operation name.
        operation: &'static str,
        /// Validation or decoding reason.
        reason: String,
    },
}

/// Narrow GitHub repository API used by the session provisioner.
#[async_trait]
pub trait GitHubRepositoryApi: Send + Sync {
    /// Creates one empty session-owned repository.
    async fn create_repository(
        &self,
        destination: &DestinationRepository,
    ) -> Result<CreatedRepository, GitHubApiError>;

    /// Grants one player, NPC, or service identity access to a destination repository.
    async fn grant_repository_access(
        &self,
        grant: &RepositoryGrant,
    ) -> Result<GrantOutcome, GitHubApiError>;

    /// Deletes one session-owned repository. A missing repository is treated as already deleted.
    async fn delete_repository(
        &self,
        destination: &DestinationRepository,
    ) -> Result<(), GitHubApiError>;
}

/// `reqwest` implementation of [`GitHubRepositoryApi`].
#[derive(Debug, Clone)]
pub struct GitHubRestClient {
    base_url: String,
    client: Client,
}

impl GitHubRestClient {
    /// Creates a REST client using a short-lived GitHub App installation token.
    ///
    /// HTTPS is required except for loopback addresses used by integration tests.
    pub fn new(base_url: &str, installation_token: &str) -> Result<Self, GitHubApiError> {
        let parsed = Url::parse(base_url)
            .map_err(|error| GitHubApiError::InvalidBaseUrl(error.to_string()))?;
        validate_base_url(&parsed)?;
        if installation_token.is_empty() {
            return Err(GitHubApiError::InvalidToken);
        }

        let mut authorization = HeaderValue::from_str(&format!("Bearer {installation_token}"))
            .map_err(|_| GitHubApiError::InvalidToken)?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            HeaderValue::from_static(GITHUB_API_VERSION),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("buzz-workplace-simulator"),
        );
        let client = Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|error| GitHubApiError::Transport(error.to_string()))?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

#[async_trait]
impl GitHubRepositoryApi for GitHubRestClient {
    async fn create_repository(
        &self,
        destination: &DestinationRepository,
    ) -> Result<CreatedRepository, GitHubApiError> {
        let operation = "create_repository";
        let response = self
            .client
            .post(self.endpoint(&format!("/orgs/{}/repos", destination.owner)))
            .json(&json!({
                "name": &destination.name,
                "private": destination.private,
                "auto_init": false,
            }))
            .send()
            .await
            .map_err(|error| GitHubApiError::Transport(error.to_string()))?;
        if response.status() != StatusCode::CREATED {
            return Err(http_status_error(operation, response).await);
        }
        let created = response
            .json::<CreatedRepository>()
            .await
            .map_err(|error| GitHubApiError::InvalidResponse {
                operation,
                reason: error.to_string(),
            })?;
        if !created.name.eq_ignore_ascii_case(&destination.name)
            || created.private != destination.private
        {
            return Err(GitHubApiError::InvalidResponse {
                operation,
                reason: "created repository does not match requested name or visibility"
                    .to_string(),
            });
        }
        Ok(created)
    }

    async fn grant_repository_access(
        &self,
        grant: &RepositoryGrant,
    ) -> Result<GrantOutcome, GitHubApiError> {
        let operation = "grant_repository_access";
        let permission = match grant.access {
            RepositoryAccess::Read => "pull",
            RepositoryAccess::Write => "push",
            RepositoryAccess::Maintain => "maintain",
        };
        let response = self
            .client
            .put(self.endpoint(&format!(
                "/repos/{}/{}/collaborators/{}",
                grant.destination_owner, grant.destination_repository, grant.github_login
            )))
            .json(&json!({"permission": permission}))
            .send()
            .await
            .map_err(|error| GitHubApiError::Transport(error.to_string()))?;
        match response.status() {
            StatusCode::CREATED => Ok(GrantOutcome::InvitationCreated),
            StatusCode::NO_CONTENT => Ok(GrantOutcome::AccessUpdated),
            _ => Err(http_status_error(operation, response).await),
        }
    }

    async fn delete_repository(
        &self,
        destination: &DestinationRepository,
    ) -> Result<(), GitHubApiError> {
        let operation = "delete_repository";
        let response = self
            .client
            .delete(self.endpoint(&format!(
                "/repos/{}/{}",
                destination.owner, destination.name
            )))
            .send()
            .await
            .map_err(|error| GitHubApiError::Transport(error.to_string()))?;
        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::NOT_FOUND => Ok(()),
            _ => Err(http_status_error(operation, response).await),
        }
    }
}

fn validate_base_url(url: &Url) -> Result<(), GitHubApiError> {
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
    });
    let valid_scheme = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    if !valid_scheme
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(GitHubApiError::InvalidBaseUrl(url.to_string()));
    }
    Ok(())
}

async fn http_status_error(operation: &'static str, response: Response) -> GitHubApiError {
    let status = response.status().as_u16();
    let bytes = response.bytes().await.unwrap_or_default();
    let bounded = &bytes[..bytes.len().min(MAX_ERROR_BODY_BYTES)];
    GitHubApiError::HttpStatus {
        operation,
        status,
        body: String::from_utf8_lossy(bounded).into_owned(),
    }
}
