use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Current wire protocol version.
pub const VERIFICATION_PROTOCOL_VERSION: u16 = 1;

/// Lifecycle state of one verification run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// The request was accepted but work has not started.
    Queued,
    /// Sources and the trusted manifest are being prepared.
    Preparing,
    /// Checks are executing.
    Running,
    /// Every required check passed.
    Passed,
    /// Player code failed one or more checks.
    Failed,
    /// Policy rejected the source before execution.
    PolicyBlocked,
    /// Runner infrastructure failed independently of player code.
    InfraError,
    /// A caller cancelled the run.
    Cancelled,
    /// A wall-clock limit expired.
    TimedOut,
}

impl RunState {
    /// Returns whether no further lifecycle transition is valid.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Passed
                | Self::Failed
                | Self::PolicyBlocked
                | Self::InfraError
                | Self::Cancelled
                | Self::TimedOut
        )
    }

    /// Converts a terminal lifecycle state into a final status.
    #[must_use]
    pub const fn final_status(self) -> Option<FinalStatus> {
        match self {
            Self::Passed => Some(FinalStatus::Passed),
            Self::Failed => Some(FinalStatus::Failed),
            Self::PolicyBlocked => Some(FinalStatus::PolicyBlocked),
            Self::InfraError => Some(FinalStatus::InfraError),
            Self::Cancelled => Some(FinalStatus::Cancelled),
            Self::TimedOut => Some(FinalStatus::TimedOut),
            Self::Queued | Self::Preparing | Self::Running => None,
        }
    }
}

/// Final objective verdict for a verification run or check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalStatus {
    /// All required behavior passed.
    Passed,
    /// Player code failed objective behavior.
    Failed,
    /// Trusted source policy rejected the change.
    PolicyBlocked,
    /// Runner infrastructure failed independently of player code.
    InfraError,
    /// A caller cancelled execution.
    Cancelled,
    /// A bounded operation exceeded its deadline.
    TimedOut,
}

impl FinalStatus {
    /// Converts this verdict to its terminal run state.
    #[must_use]
    pub const fn run_state(self) -> RunState {
        match self {
            Self::Passed => RunState::Passed,
            Self::Failed => RunState::Failed,
            Self::PolicyBlocked => RunState::PolicyBlocked,
            Self::InfraError => RunState::InfraError,
            Self::Cancelled => RunState::Cancelled,
            Self::TimedOut => RunState::TimedOut,
        }
    }
}

/// Exact repository revision submitted for verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryRevision {
    /// Stable scenario-local repository identifier.
    pub repository_id: String,
    /// Allowlisted HTTPS or operator-approved file URL.
    pub clone_url: String,
    /// Full base Git object identifier.
    pub base_commit_sha: String,
    /// Full head Git object identifier.
    pub head_commit_sha: String,
}

/// Verification request accepted by the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationRequest {
    /// Wire protocol version.
    pub version: u16,
    /// Caller-selected idempotency key.
    pub run_id: Uuid,
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// Trusted scenario identifier.
    pub scenario_id: String,
    /// Immutable scenario version.
    pub scenario_version: String,
    /// SHA-256 of the trusted manifest expected by the caller.
    pub expected_manifest_digest: String,
    /// Exact repository revisions to verify.
    pub repositories: Vec<RepositoryRevision>,
}

/// Immediate response returned after accepting a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationAccepted {
    /// Wire protocol version.
    pub version: u16,
    /// Accepted run identifier.
    pub run_id: Uuid,
    /// Semantic request fingerprint.
    pub request_digest: String,
    /// Initial or replayed lifecycle state.
    pub state: RunState,
}

/// Objective phase represented by a check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckPhase {
    /// Source and repository policy.
    Policy,
    /// Compilation or package build.
    Build,
    /// Unit tests.
    Unit,
    /// Cross-component integration tests.
    Integration,
    /// Service startup and readiness.
    Service,
    /// Host-side HTTP behavior probes.
    Probe,
    /// Runtime observation after startup.
    Runtime,
}

/// Visibility of evidence and checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceVisibility {
    /// Safe to show to the player.
    Player,
    /// Available only to trusted evaluators.
    EvaluatorOnly,
}

/// One objective assertion produced by a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertionResult {
    /// Stable machine-readable assertion key.
    pub key: String,
    /// Whether the assertion passed.
    pub passed: bool,
    /// Expected JSON value when safe and relevant.
    pub expected: Option<Value>,
    /// Observed JSON value when safe and relevant.
    pub observed: Option<Value>,
    /// Human-readable detail for trusted evaluation.
    pub message: Option<String>,
}

/// Content-addressed evidence artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    /// Stable artifact name.
    pub name: String,
    /// SHA-256 of artifact bytes.
    pub sha256: String,
    /// Exact byte length.
    pub byte_len: u64,
    /// Player or evaluator-only visibility.
    pub visibility: EvidenceVisibility,
    /// Runner-local relative path, omitted from public projections.
    pub path: Option<String>,
}

/// Immutable execution environment evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentEvidence {
    /// SHA-256 of the trusted scenario manifest.
    pub manifest_digest: String,
    /// Human-readable immutable image reference.
    pub image_reference: String,
    /// Resolved container image digest.
    pub image_digest: String,
    /// Backend implementation identifier.
    pub backend: String,
}

/// Exact repository identity observed by the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedRepository {
    /// Stable scenario-local repository identifier.
    pub repository_id: String,
    /// Full base Git object identifier.
    pub base_commit_sha: String,
    /// Full verified head Git object identifier.
    pub head_commit_sha: String,
    /// Sorted changed paths between base and head.
    pub changed_paths: Vec<String>,
}

/// Result of one objective verification check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    /// Stable check identifier.
    pub id: String,
    /// Objective phase.
    pub phase: CheckPhase,
    /// Final check verdict.
    pub status: FinalStatus,
    /// Player or evaluator-only visibility.
    pub visibility: EvidenceVisibility,
    /// Stable objective assertions.
    pub assertions: Vec<AssertionResult>,
    /// Captured stdout artifact when available.
    pub stdout_artifact: Option<ArtifactRef>,
    /// Captured stderr artifact when available.
    pub stderr_artifact: Option<ArtifactRef>,
    /// Observed wall-clock duration; excluded from normalized digests.
    pub duration_ms: u64,
}

/// Stable failure category plus optional human-readable explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureSummary {
    /// Stable failure category.
    pub code: String,
    /// Human-readable text; excluded from normalized digests.
    pub message: String,
}

/// Full evaluator-facing verification result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationResult {
    /// Wire protocol version.
    pub version: u16,
    /// Run identifier.
    pub run_id: Uuid,
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// Trusted scenario identifier.
    pub scenario_id: String,
    /// Immutable scenario version.
    pub scenario_version: String,
    /// Objective final verdict.
    pub status: FinalStatus,
    /// Semantic request fingerprint.
    pub request_digest: String,
    /// Exact repository set fingerprint.
    pub commit_set_digest: String,
    /// Exact verified repositories.
    pub repositories: Vec<VerifiedRepository>,
    /// Immutable environment evidence.
    pub environment: EnvironmentEvidence,
    /// Deterministic semantic result fingerprint.
    pub normalized_result_digest: String,
    /// Observed start timestamp.
    pub started_at: DateTime<Utc>,
    /// Observed finish timestamp.
    pub finished_at: DateTime<Utc>,
    /// Full public and hidden check evidence.
    pub checks: Vec<CheckResult>,
    /// Full public and hidden artifact index.
    pub artifacts: Vec<ArtifactRef>,
    /// Optional failure explanation.
    pub failure: Option<FailureSummary>,
}

/// Player-visible summary of a public check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicCheckSummary {
    /// Public check identifier.
    pub id: String,
    /// Objective phase.
    pub phase: CheckPhase,
    /// Final check verdict.
    pub status: FinalStatus,
    /// Public assertions.
    pub assertions: Vec<AssertionResult>,
}

/// Aggregate status counts for hidden checks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HiddenCheckSummary {
    /// Total evaluator-only checks.
    pub total: u32,
    /// Hidden checks that passed.
    pub passed: u32,
    /// Hidden checks that failed player behavior.
    pub failed: u32,
    /// Hidden checks blocked by policy.
    pub policy_blocked: u32,
    /// Hidden checks affected by infrastructure failure.
    pub infra_error: u32,
    /// Hidden checks cancelled.
    pub cancelled: u32,
    /// Hidden checks that timed out.
    pub timed_out: u32,
}

impl HiddenCheckSummary {
    fn record(&mut self, status: FinalStatus) {
        self.total += 1;
        match status {
            FinalStatus::Passed => self.passed += 1,
            FinalStatus::Failed => self.failed += 1,
            FinalStatus::PolicyBlocked => self.policy_blocked += 1,
            FinalStatus::InfraError => self.infra_error += 1,
            FinalStatus::Cancelled => self.cancelled += 1,
            FinalStatus::TimedOut => self.timed_out += 1,
        }
    }
}

/// Player-safe verification projection suitable for Buzz publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicVerificationResult {
    /// Wire protocol version.
    pub version: u16,
    /// Run identifier.
    pub run_id: Uuid,
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// Trusted scenario identifier.
    pub scenario_id: String,
    /// Immutable scenario version.
    pub scenario_version: String,
    /// Objective final verdict.
    pub status: FinalStatus,
    /// Semantic request fingerprint.
    pub request_digest: String,
    /// Exact repository set fingerprint.
    pub commit_set_digest: String,
    /// Exact verified repositories.
    pub repositories: Vec<VerifiedRepository>,
    /// Immutable execution environment evidence.
    pub environment: EnvironmentEvidence,
    /// Deterministic semantic result fingerprint.
    pub normalized_result_digest: String,
    /// Player-visible checks.
    pub public_checks: Vec<PublicCheckSummary>,
    /// Aggregate evaluator-only status counts.
    pub hidden_checks: HiddenCheckSummary,
    /// Player-visible artifacts with runner paths removed.
    pub player_artifacts: Vec<ArtifactRef>,
    /// Player-safe failure category.
    pub failure: Option<FailureSummary>,
}

impl VerificationResult {
    /// Creates a projection that cannot expose evaluator-only identifiers or evidence.
    #[must_use]
    pub fn public_projection(&self) -> PublicVerificationResult {
        let mut hidden_checks = HiddenCheckSummary::default();
        let mut public_checks = Vec::new();
        for check in &self.checks {
            match check.visibility {
                EvidenceVisibility::Player => public_checks.push(PublicCheckSummary {
                    id: check.id.clone(),
                    phase: check.phase,
                    status: check.status,
                    assertions: check.assertions.clone(),
                }),
                EvidenceVisibility::EvaluatorOnly => hidden_checks.record(check.status),
            }
        }
        public_checks.sort_by(|left, right| left.id.cmp(&right.id));

        let mut player_artifacts = self
            .artifacts
            .iter()
            .filter(|artifact| artifact.visibility == EvidenceVisibility::Player)
            .cloned()
            .map(|mut artifact| {
                artifact.path = None;
                artifact
            })
            .collect::<Vec<_>>();
        player_artifacts.sort_by(|left, right| left.name.cmp(&right.name));

        let failure = self.failure.as_ref().map(|failure| FailureSummary {
            code: failure.code.clone(),
            message: public_failure_message(&failure.code).to_owned(),
        });

        PublicVerificationResult {
            version: self.version,
            run_id: self.run_id,
            session_id: self.session_id,
            scenario_id: self.scenario_id.clone(),
            scenario_version: self.scenario_version.clone(),
            status: self.status,
            request_digest: self.request_digest.clone(),
            commit_set_digest: self.commit_set_digest.clone(),
            repositories: self.repositories.clone(),
            environment: self.environment.clone(),
            normalized_result_digest: self.normalized_result_digest.clone(),
            public_checks,
            hidden_checks,
            player_artifacts,
            failure,
        }
    }
}

fn public_failure_message(code: &str) -> &'static str {
    match code {
        "policy_blocked" => "The submitted source violated scenario policy.",
        "infra_error" => "Verification infrastructure failed.",
        "cancelled" => "Verification was cancelled.",
        "timed_out" => "Verification exceeded its time limit.",
        _ => "One or more verification checks failed.",
    }
}
