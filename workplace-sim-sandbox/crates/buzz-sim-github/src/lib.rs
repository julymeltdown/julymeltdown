#![deny(unsafe_code)]
//! Deterministic contracts for provisioning isolated GitHub repositories per simulation session.
//!
//! This crate intentionally contains no GitHub token and performs no network calls. It validates
//! exact source revisions, compiles least-privilege actor grants, and produces stable seed plans
//! that a GitHub App adapter can execute.

mod actor;
mod seed;
mod session;

pub use actor::{
    ActorBinding, ActorDirectory, ActorKind, RepositoryAccess, ResolvedActor,
};
pub use seed::{
    CredentialAccess, DestinationRepository, GitCredentialScope, SeedOperation, SeedPlan,
    SourceRevision,
};
pub use session::{
    RepositoryGrant, SessionProvisioningPlan, SessionProvisioningSpec, SessionRepositorySpec,
};

/// Validation and compilation failures for GitHub session provisioning.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProvisioningError {
    /// A stable simulator identifier was empty or contained unsupported characters.
    #[error("invalid {field} {value:?}; expected [A-Za-z0-9._:-] with no leading dot or '..'")]
    InvalidIdentifier {
        /// Name of the invalid field.
        field: &'static str,
        /// Rejected value.
        value: String,
    },
    /// A GitHub login violated GitHub's portable username rules.
    #[error("invalid GitHub login {0:?}")]
    InvalidGitHubLogin(String),
    /// A GitHub owner or repository name was invalid.
    #[error("invalid GitHub {field} {value:?}")]
    InvalidGitHubName {
        /// Name of the invalid field.
        field: &'static str,
        /// Rejected value.
        value: String,
    },
    /// A Git reference name was unsafe or malformed.
    #[error("invalid Git ref name {0:?}")]
    InvalidGitRef(String),
    /// A source clone URL was not an uncredentialed HTTPS repository URL.
    #[error("invalid clone URL {url:?}: {reason}")]
    InvalidCloneUrl {
        /// Rejected URL.
        url: String,
        /// Stable human-readable reason.
        reason: String,
    },
    /// A commit identifier was abbreviated or malformed.
    #[error("commit id must be a full 40-character SHA-1 or 64-character SHA-256 hex value")]
    InvalidCommitSha,
    /// A source and destination used different scenario-local repository identifiers.
    #[error("repository id mismatch: source={source:?}, destination={destination:?}")]
    RepositoryIdMismatch {
        /// Source identifier.
        source: String,
        /// Destination identifier.
        destination: String,
    },
    /// A seed plan attempted to write back into its source repository.
    #[error("source and destination resolve to the same repository {0}")]
    SourceEqualsDestination(String),
    /// Two actor definitions used the same stable actor identifier.
    #[error("duplicate actor id {0:?}")]
    DuplicateActorId(String),
    /// Two actor definitions used the same canonical GitHub login.
    #[error("duplicate GitHub login {0:?}")]
    DuplicateGitHubLogin(String),
    /// Two session repository specifications used the same local repository identifier.
    #[error("duplicate repository id {0:?}")]
    DuplicateRepositoryId(String),
    /// Two session repository specifications targeted the same destination repository.
    #[error("duplicate destination repository {0}")]
    DuplicateDestination(String),
    /// An actor requested access to a repository not present in the session.
    #[error("actor {actor_id:?} references unknown repository {repository_id:?}")]
    UnknownRepository {
        /// Actor that requested the grant.
        actor_id: String,
        /// Missing repository identifier.
        repository_id: String,
    },
    /// A verification projection omitted a repository head commit.
    #[error("missing verified head commit for repository {0:?}")]
    MissingHeadCommit(String),
    /// A supplied credential scope did not match its exact repository and access purpose.
    #[error("credential scope mismatch: expected {expected}, got {actual}")]
    CredentialScopeMismatch {
        /// Required scope summary.
        expected: String,
        /// Actual scope summary.
        actual: String,
    },
    /// A session contained no repositories.
    #[error("a simulation session must provision at least one repository")]
    EmptyRepositorySet,
}

pub(crate) fn validate_identifier(
    field: &'static str,
    value: &str,
) -> Result<(), ProvisioningError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('.')
        && !value.contains("..")
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:-".contains(character));
    if valid {
        Ok(())
    } else {
        Err(ProvisioningError::InvalidIdentifier {
            field,
            value: value.to_string(),
        })
    }
}

pub(crate) fn validate_full_git_object_id(value: &str) -> Result<(), ProvisioningError> {
    if (value.len() == 40 || value.len() == 64)
        && value.chars().all(|character| character.is_ascii_hexdigit())
    {
        Ok(())
    } else {
        Err(ProvisioningError::InvalidCommitSha)
    }
}
