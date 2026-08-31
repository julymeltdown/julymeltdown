use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use buzz_sim_github::{
    CredentialAccess, DestinationRepository, GitCliSeeder, GitCommand, GitCommandExecutor,
    GitCommandOutput, GitCommandPhase, GitCredentialLease, GitCredentialProvider,
    GitCredentialScope, GitHubApiError, RepositorySeeder, SeedPlan, SourceRevision,
    TokioGitCommandExecutor,
};
use uuid::Uuid;

const SOURCE_SHA: &str = "1111111111111111111111111111111111111111";
const OTHER_SHA: &str = "2222222222222222222222222222222222222222";

#[derive(Debug, Clone, Default)]
struct ProviderLog(Arc<Mutex<Vec<(String, String, CredentialAccess)>>>);

#[derive(Debug, Clone)]
struct FakeCredentialProvider {
    calls: ProviderLog,
}

#[async_trait]
impl GitCredentialProvider for FakeCredentialProvider {
    async fn credential_for(
        &self,
        scope: &GitCredentialScope,
    ) -> Result<GitCredentialLease, GitHubApiError> {
        self.calls.0.lock().unwrap().push((
            scope.owner.clone(),
            scope.repository.clone(),
            scope.access,
        ));
        let secret = match scope.access {
            CredentialAccess::Read => "secret-source",
            CredentialAccess::Write => "secret-destination",
        };
        GitCredentialLease::new("x-access-token", secret)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutedCommand {
    phase: GitCommandPhase,
    arguments: Vec<String>,
    credential_username: Option<String>,
}

#[derive(Debug, Clone)]
struct FakeCommandExecutor {
    calls: Arc<Mutex<Vec<ExecutedCommand>>>,
    resolved_sha: String,
}

#[async_trait]
impl GitCommandExecutor for FakeCommandExecutor {
    async fn execute(
        &self,
        _working_directory: &Path,
        command: &GitCommand,
        credential: Option<&GitCredentialLease>,
    ) -> Result<GitCommandOutput, GitHubApiError> {
        self.calls.lock().unwrap().push(ExecutedCommand {
            phase: command.phase,
            arguments: command.arguments.clone(),
            credential_username: credential.map(|value| value.username().to_string()),
        });
        let stdout = if command.phase == GitCommandPhase::ResolveFetchedCommit {
            format!("{}\n", self.resolved_sha)
        } else {
            String::new()
        };
        Ok(GitCommandOutput {
            stdout,
            stderr: String::new(),
        })
    }
}

fn seed_plan() -> SeedPlan {
    let source = SourceRevision::new(
        "legacy-cart",
        "https://github.com/acme/legacy-cart.git",
        SOURCE_SHA,
    )
    .unwrap();
    let source_scope = GitCredentialScope::for_source(&source).unwrap();
    let destination = DestinationRepository::new(
        "legacy-cart",
        "acme-sim",
        "session-legacy-cart",
        "main",
        true,
    )
    .unwrap();
    SeedPlan::new(
        Uuid::parse_str("00000000-0000-4000-8000-000000000099").unwrap(),
        source,
        destination,
        Some(source_scope),
    )
    .unwrap()
}

#[tokio::test]
async fn git_cli_seeder_fetches_and_pushes_only_the_exact_commit() {
    let provider_log = ProviderLog::default();
    let command_log = Arc::new(Mutex::new(Vec::new()));
    let seeder = GitCliSeeder::new(
        FakeCredentialProvider {
            calls: provider_log.clone(),
        },
        FakeCommandExecutor {
            calls: command_log.clone(),
            resolved_sha: SOURCE_SHA.to_string(),
        },
    );

    let seeded = seeder.seed_repository(&seed_plan()).await.unwrap();

    assert_eq!(seeded.repository_id, "legacy-cart");
    assert_eq!(seeded.head_commit_sha, SOURCE_SHA);
    assert_eq!(
        provider_log.0.lock().unwrap().clone(),
        vec![
            (
                "acme".to_string(),
                "legacy-cart".to_string(),
                CredentialAccess::Read,
            ),
            (
                "acme-sim".to_string(),
                "session-legacy-cart".to_string(),
                CredentialAccess::Write,
            ),
        ]
    );

    let calls = command_log.lock().unwrap().clone();
    assert_eq!(
        calls.iter().map(|call| call.phase).collect::<Vec<_>>(),
        vec![
            GitCommandPhase::Initialize,
            GitCommandPhase::AddSourceRemote,
            GitCommandPhase::FetchExactCommit,
            GitCommandPhase::ResolveFetchedCommit,
            GitCommandPhase::AddDestinationRemote,
            GitCommandPhase::PushExactCommit,
        ]
    );
    assert_eq!(
        calls[2].credential_username.as_deref(),
        Some("x-access-token")
    );
    assert_eq!(
        calls[5].credential_username.as_deref(),
        Some("x-access-token")
    );
    assert!(calls[2].arguments.contains(&SOURCE_SHA.to_string()));
    assert!(calls[5]
        .arguments
        .contains(&format!("{SOURCE_SHA}:refs/heads/main")));
    assert!(calls[5]
        .arguments
        .contains(&"--force-with-lease=refs/heads/main:".to_string()));
    assert!(calls
        .iter()
        .flat_map(|call| &call.arguments)
        .all(|argument| {
            !argument.contains("secret-source") && !argument.contains("secret-destination")
        }));
}

#[tokio::test]
async fn fetched_commit_mismatch_stops_before_destination_remote_or_push() {
    let command_log = Arc::new(Mutex::new(Vec::new()));
    let seeder = GitCliSeeder::new(
        FakeCredentialProvider {
            calls: ProviderLog::default(),
        },
        FakeCommandExecutor {
            calls: command_log.clone(),
            resolved_sha: OTHER_SHA.to_string(),
        },
    );

    let error = seeder.seed_repository(&seed_plan()).await.unwrap_err();

    assert!(matches!(
        error,
        GitHubApiError::InvalidResponse {
            operation: "seed_repository",
            reason,
        } if reason.contains("fetched commit")
    ));
    assert_eq!(command_log.lock().unwrap().len(), 4);
}

#[test]
fn credential_debug_output_never_contains_the_secret() {
    let credential = GitCredentialLease::new("x-access-token", "top-secret-token").unwrap();
    let debug = format!("{credential:?}");

    assert!(debug.contains("x-access-token"));
    assert!(!debug.contains("top-secret-token"));
    assert!(debug.contains("REDACTED"));
}

#[tokio::test]
async fn tokio_executor_runs_a_real_git_process_without_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let command = GitCommand::new(GitCommandPhase::Probe, ["--version"]);
    let output = TokioGitCommandExecutor
        .execute(directory.path(), &command, None)
        .await
        .unwrap();

    assert!(output.stdout.starts_with("git version "));
    assert!(output.stderr.is_empty());
}
