use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use buzz_sim_agent::{
    ConversationSurface, MemoryLedger, NpcActionDraft, NpcModel, NpcModelInput, NpcModelOutput,
    NpcOrchestrator, NpcReplyDraft, NpcTurnRequest, PersonaDirectory, PersonaPack,
    ValidatedNpcTurn, WorldSnapshot,
};
use buzz_sim_executor::{
    BuzzDestination, BuzzGateway, BuzzMessageCommand, BuzzMessageReceipt, ExecutedNpcTurn,
    ExecutionContext, ExecutionError, GatewayFailure, GatewayKind, GitHubCommand, GitHubGateway,
    GitHubReceipt, MemoryExecutionLedger, NpcActionExecutor, OperationReceipt,
    RepositoryExecutionTarget, SimulationCommand, SimulationGateway, SimulationReceipt,
    VerificationCommand, VerificationGateway,
};
use buzz_sim_github::{
    ActorBinding, ActorDirectory, ActorKind, DestinationRepository, RepositoryAccess,
};
use buzz_sim_protocol::{RunState, VerificationAccepted, VERIFICATION_PROTOCOL_VERSION};
use uuid::Uuid;

const SESSION_ID: &str = "00000000-0000-4000-8000-000000000701";
const TURN_ID: &str = "00000000-0000-4000-8000-000000000702";
const BASE_SHA: &str = "1111111111111111111111111111111111111111";
const HEAD_SHA: &str = "2222222222222222222222222222222222222222";
const MANIFEST_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const PERSONAS: &str = r#"
version: 1
personas:
  - id: minseo
    display_name: 문민서
    presentation: woman
    role: staff_backend_engineer
    team: checkout
    public_traits: [무뚝뚝함]
    private_traits: [과거 장애 기록을 경계함]
    speech_style: [짧은 문장]
    goals:
      - id: preserve_v1
        description: API v1 호환성을 지킨다
        priority: 90
    capabilities:
      - send_message
      - create_branch
      - request_review
      - open_pull_request
      - review_pull_request
      - run_verification
      - escalate
      - schedule_meeting
    channels: [checkout-team]
    repository_access:
      legacy-cart: maintain
    workload: 40
    availability: available
    knowledge:
      - id: mobile_v1
        statement: 모바일 앱은 API v1을 사용한다
        stance: fact
        disclosure: team
  - id: chaewon
    display_name: 강채원
    presentation: woman
    role: engineering_manager
    team: checkout
    public_traits: [침착함]
    private_traits: [팀 존속을 우선함]
    speech_style: [정중한 문장]
    goals:
      - id: protect_team
        description: 팀을 보호한다
        priority: 95
    capabilities: [send_message, review_pull_request, escalate, schedule_meeting]
    channels: [checkout-team]
    repository_access:
      legacy-cart: read
    workload: 65
    availability: available
    knowledge:
      - id: team_risk
        statement: 팀은 조직 개편 후보이다
        stance: fact
        disclosure: discretionary
"#;

#[derive(Debug, Clone)]
struct StaticModel {
    output: NpcModelOutput,
}

#[async_trait]
impl NpcModel for StaticModel {
    async fn generate(
        &self,
        _input: &NpcModelInput,
    ) -> Result<NpcModelOutput, buzz_sim_agent::AgentError> {
        Ok(self.output.clone())
    }
}

#[derive(Debug, Clone, Default)]
struct CallLog(Arc<Mutex<Vec<String>>>);

impl CallLog {
    fn push(&self, value: impl Into<String>) {
        self.0.lock().unwrap().push(value.into());
    }

    fn values(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Debug, Clone, Default)]
struct FakeBuzz {
    log: CallLog,
    commands: Arc<Mutex<Vec<BuzzMessageCommand>>>,
}

#[async_trait]
impl BuzzGateway for FakeBuzz {
    async fn send_message(
        &mut self,
        command: &BuzzMessageCommand,
    ) -> Result<BuzzMessageReceipt, GatewayFailure> {
        let label = match &command.destination {
            BuzzDestination::DirectMessage { .. } => "buzz:reply",
            BuzzDestination::Channel { .. } => "buzz:channel",
        };
        self.log.push(label);
        self.commands.lock().unwrap().push(command.clone());
        Ok(BuzzMessageReceipt {
            message_id: format!("message-{}", self.commands.lock().unwrap().len()),
        })
    }
}

#[derive(Debug, Clone, Default)]
struct FakeGitHub {
    log: CallLog,
    commands: Arc<Mutex<Vec<GitHubCommand>>>,
    fail_create_branch_once: Arc<Mutex<bool>>,
}

#[async_trait]
impl GitHubGateway for FakeGitHub {
    async fn execute(&mut self, command: &GitHubCommand) -> Result<GitHubReceipt, GatewayFailure> {
        let label = match command {
            GitHubCommand::CreateBranch { .. } => "github:create_branch",
            GitHubCommand::RequestReview { .. } => "github:request_review",
            GitHubCommand::OpenPullRequest { .. } => "github:open_pull_request",
            GitHubCommand::ReviewPullRequest { .. } => "github:review_pull_request",
        };
        self.log.push(label);
        self.commands.lock().unwrap().push(command.clone());

        if matches!(command, GitHubCommand::CreateBranch { .. }) {
            let mut fail = self.fail_create_branch_once.lock().unwrap();
            if *fail {
                *fail = false;
                return Err(GatewayFailure::retryable(
                    GatewayKind::GitHub,
                    "temporary_transport",
                    "connection reset",
                ));
            }
        }

        Ok(match command {
            GitHubCommand::CreateBranch {
                branch_name,
                from_sha,
                ..
            } => GitHubReceipt::BranchCreated {
                branch_name: branch_name.clone(),
                commit_sha: from_sha.clone(),
            },
            GitHubCommand::RequestReview {
                pull_request,
                reviewer_logins,
                ..
            } => GitHubReceipt::ReviewRequested {
                pull_request: *pull_request,
                reviewer_logins: reviewer_logins.clone(),
            },
            GitHubCommand::OpenPullRequest { .. } => GitHubReceipt::PullRequestOpened {
                pull_request: 41,
                url: "https://github.com/momo-sim/sim-session-legacy-cart/pull/41".to_string(),
            },
            GitHubCommand::ReviewPullRequest { pull_request, .. } => {
                GitHubReceipt::PullRequestReviewed {
                    pull_request: *pull_request,
                    review_id: 91,
                }
            }
        })
    }
}

#[derive(Debug, Clone, Default)]
struct FakeVerification {
    log: CallLog,
    commands: Arc<Mutex<Vec<VerificationCommand>>>,
}

#[async_trait]
impl VerificationGateway for FakeVerification {
    async fn submit(
        &mut self,
        command: &VerificationCommand,
    ) -> Result<VerificationAccepted, GatewayFailure> {
        self.log.push("verification:submit");
        self.commands.lock().unwrap().push(command.clone());
        Ok(VerificationAccepted {
            version: VERIFICATION_PROTOCOL_VERSION,
            run_id: command.request.run_id,
            request_digest: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_string(),
            state: RunState::Queued,
        })
    }
}

#[derive(Debug, Clone, Default)]
struct FakeSimulation {
    log: CallLog,
    commands: Arc<Mutex<Vec<SimulationCommand>>>,
}

#[async_trait]
impl SimulationGateway for FakeSimulation {
    async fn apply(
        &mut self,
        command: &SimulationCommand,
    ) -> Result<SimulationReceipt, GatewayFailure> {
        let label = match command {
            SimulationCommand::Escalate { .. } => "simulation:escalate",
            SimulationCommand::ScheduleMeeting { .. } => "simulation:meeting",
        };
        self.log.push(label);
        self.commands.lock().unwrap().push(command.clone());
        Ok(SimulationReceipt {
            event_id: Uuid::from_u128(900 + self.commands.lock().unwrap().len() as u128),
        })
    }
}

fn personas() -> PersonaDirectory {
    PersonaDirectory::new(PersonaPack::from_yaml(PERSONAS).unwrap()).unwrap()
}

fn actor_directory() -> ActorDirectory {
    ActorDirectory::new([
        ActorBinding::new(
            "player",
            "player-dev",
            ActorKind::Player,
            BTreeMap::from([("legacy-cart".to_string(), RepositoryAccess::Write)]),
        )
        .unwrap(),
        ActorBinding::new(
            "minseo",
            "minseo-bot",
            ActorKind::Npc,
            BTreeMap::from([("legacy-cart".to_string(), RepositoryAccess::Maintain)]),
        )
        .unwrap(),
        ActorBinding::new(
            "chaewon",
            "chaewon-bot",
            ActorKind::Npc,
            BTreeMap::from([("legacy-cart".to_string(), RepositoryAccess::Read)]),
        )
        .unwrap(),
    ])
    .unwrap()
}

fn repository_target() -> RepositoryExecutionTarget {
    RepositoryExecutionTarget::new(
        DestinationRepository::new(
            "legacy-cart",
            "momo-sim",
            "sim-session-legacy-cart",
            "main",
            true,
        )
        .unwrap(),
        BASE_SHA,
        HEAD_SHA,
        MANIFEST_DIGEST,
    )
    .unwrap()
}

fn context(review_routes: BTreeMap<String, BTreeSet<String>>) -> ExecutionContext {
    ExecutionContext::new(
        Uuid::parse_str(SESSION_ID).unwrap(),
        "player",
        "momo-commerce-season-1",
        "1.0.0",
        personas(),
        actor_directory(),
        [repository_target()],
        review_routes,
    )
    .unwrap()
}

async fn validated_turn(
    reply: Option<NpcReplyDraft>,
    actions: Vec<NpcActionDraft>,
) -> ValidatedNpcTurn {
    let request = NpcTurnRequest {
        session_id: Uuid::parse_str(SESSION_ID).unwrap(),
        turn_id: Uuid::parse_str(TURN_ID).unwrap(),
        actor_id: "minseo".to_string(),
        player_input: "현재 위험을 확인하고 필요한 업무를 진행해 줘.".to_string(),
        surface: ConversationSurface::DirectMessage,
        world: WorldSnapshot {
            week: 3,
            sprint: 2,
            work_block: 81,
            active_incident: None,
            visible_facts: BTreeMap::new(),
        },
    };
    NpcOrchestrator::new(
        personas(),
        MemoryLedger::default(),
        StaticModel {
            output: NpcModelOutput {
                reply,
                actions,
                memory_note: None,
            },
        },
    )
    .orchestrate(&request, 16)
    .await
    .unwrap()
}

fn all_actions() -> Vec<NpcActionDraft> {
    vec![
        NpcActionDraft::SendMessage {
            channel_id: "checkout-team".to_string(),
            body: "모바일 계약부터 확인합니다.".to_string(),
            fact_ids: BTreeSet::from(["mobile_v1".to_string()]),
        },
        NpcActionDraft::CreateBranch {
            repository_id: "legacy-cart".to_string(),
            branch_name: "sim/minseo/coupon-fix".to_string(),
            purpose: "쿠폰 계산 수정".to_string(),
        },
        NpcActionDraft::RequestReview {
            repository_id: "legacy-cart".to_string(),
            pull_request: 7,
        },
        NpcActionDraft::OpenPullRequest {
            repository_id: "legacy-cart".to_string(),
            branch_name: "sim/minseo/coupon-fix".to_string(),
            title: "쿠폰 계산 수정".to_string(),
            body: "API v1 계약을 유지합니다.".to_string(),
        },
        NpcActionDraft::ReviewPullRequest {
            repository_id: "legacy-cart".to_string(),
            pull_request: 7,
            body: "계약 테스트를 추가해 주세요.".to_string(),
        },
        NpcActionDraft::RunVerification {
            repository_id: "legacy-cart".to_string(),
            commit_sha: HEAD_SHA.to_string(),
            manifest_digest: MANIFEST_DIGEST.to_string(),
        },
        NpcActionDraft::Escalate {
            target_actor_id: "chaewon".to_string(),
            summary: "모바일 계약 위험을 일정에 반영해야 합니다.".to_string(),
        },
        NpcActionDraft::ScheduleMeeting {
            participant_actor_ids: BTreeSet::from(["chaewon".to_string()]),
            agenda: "API v1 호환성 검토".to_string(),
            duration_blocks: 2,
        },
    ]
}

type TestExecutor = NpcActionExecutor<
    FakeBuzz,
    FakeGitHub,
    FakeVerification,
    FakeSimulation,
    MemoryExecutionLedger,
>;
type SharedCommands<T> = Arc<Mutex<Vec<T>>>;
type ExecutorFixture = (
    TestExecutor,
    SharedCommands<BuzzMessageCommand>,
    SharedCommands<GitHubCommand>,
    SharedCommands<VerificationCommand>,
);

fn executor(log: &CallLog, fail_create_branch_once: bool) -> ExecutorFixture {
    let buzz_commands = Arc::new(Mutex::new(Vec::new()));
    let github_commands = Arc::new(Mutex::new(Vec::new()));
    let verification_commands = Arc::new(Mutex::new(Vec::new()));
    let simulation_commands = Arc::new(Mutex::new(Vec::new()));
    let executor = NpcActionExecutor::new(
        FakeBuzz {
            log: log.clone(),
            commands: buzz_commands.clone(),
        },
        FakeGitHub {
            log: log.clone(),
            commands: github_commands.clone(),
            fail_create_branch_once: Arc::new(Mutex::new(fail_create_branch_once)),
        },
        FakeVerification {
            log: log.clone(),
            commands: verification_commands.clone(),
        },
        FakeSimulation {
            log: log.clone(),
            commands: simulation_commands,
        },
        MemoryExecutionLedger::default(),
    );
    (
        executor,
        buzz_commands,
        github_commands,
        verification_commands,
    )
}

#[tokio::test]
async fn dispatches_reply_and_actions_in_deterministic_order() {
    let turn = validated_turn(
        Some(NpcReplyDraft {
            body: "모바일 앱이 v1을 사용합니다.".to_string(),
            fact_ids: BTreeSet::from(["mobile_v1".to_string()]),
        }),
        all_actions(),
    )
    .await;
    let context = context(BTreeMap::from([(
        "legacy-cart".to_string(),
        BTreeSet::from(["chaewon".to_string()]),
    )]));
    let log = CallLog::default();
    let (mut executor, buzz_commands, github_commands, verification_commands) =
        executor(&log, false);

    let executed = executor
        .execute_turn(&context, &turn, &ConversationSurface::DirectMessage)
        .await
        .unwrap();

    assert_eq!(executed.operations.len(), 9);
    assert!(executed
        .operations
        .iter()
        .all(|operation| !operation.replayed));
    assert_eq!(
        log.values(),
        vec![
            "buzz:reply",
            "buzz:channel",
            "github:create_branch",
            "github:request_review",
            "github:open_pull_request",
            "github:review_pull_request",
            "verification:submit",
            "simulation:escalate",
            "simulation:meeting",
        ]
    );

    let buzz = buzz_commands.lock().unwrap();
    assert!(matches!(
        &buzz[0].destination,
        BuzzDestination::DirectMessage { recipient_actor_id } if recipient_actor_id == "player"
    ));
    assert!(matches!(
        &buzz[1].destination,
        BuzzDestination::Channel { channel_id } if channel_id == "checkout-team"
    ));

    let github = github_commands.lock().unwrap();
    assert!(matches!(
        &github[0],
        GitHubCommand::CreateBranch {
            actor_login,
            from_sha,
            repository_id,
            ..
        } if actor_login == "minseo-bot" && from_sha == HEAD_SHA && repository_id == "legacy-cart"
    ));
    assert!(matches!(
        &github[1],
        GitHubCommand::RequestReview { reviewer_logins, .. }
            if reviewer_logins == &BTreeSet::from(["chaewon-bot".to_string()])
    ));

    let verification = verification_commands.lock().unwrap();
    assert_eq!(
        verification[0].request.session_id,
        Uuid::parse_str(SESSION_ID).unwrap()
    );
    assert_eq!(
        verification[0].request.scenario_id,
        "momo-commerce-season-1"
    );
    assert_eq!(verification[0].request.scenario_version, "1.0.0");
    assert_eq!(verification[0].request.repositories.len(), 1);
    assert_eq!(
        verification[0].request.repositories[0].base_commit_sha,
        BASE_SHA
    );
    assert_eq!(
        verification[0].request.repositories[0].head_commit_sha,
        HEAD_SHA
    );
    assert!(matches!(
        executed.operations[6].receipt,
        OperationReceipt::Verification(_)
    ));
}

#[tokio::test]
async fn retries_replay_completed_receipts_without_repeating_side_effects() {
    let turn = validated_turn(
        Some(NpcReplyDraft {
            body: "확인했습니다.".to_string(),
            fact_ids: BTreeSet::new(),
        }),
        vec![NpcActionDraft::SendMessage {
            channel_id: "checkout-team".to_string(),
            body: "확인 중입니다.".to_string(),
            fact_ids: BTreeSet::new(),
        }],
    )
    .await;
    let context = context(BTreeMap::new());
    let log = CallLog::default();
    let (mut executor, _, _, _) = executor(&log, false);

    let first = executor
        .execute_turn(&context, &turn, &ConversationSurface::DirectMessage)
        .await
        .unwrap();
    let second = executor
        .execute_turn(&context, &turn, &ConversationSurface::DirectMessage)
        .await
        .unwrap();

    assert_eq!(first.operations.len(), 2);
    assert_eq!(second.operations.len(), 2);
    assert!(second.operations.iter().all(|operation| operation.replayed));
    assert_eq!(log.values(), vec!["buzz:reply", "buzz:channel"]);
}

#[tokio::test]
async fn transient_failure_stops_later_actions_and_retry_resumes() {
    let turn = validated_turn(
        None,
        vec![
            NpcActionDraft::SendMessage {
                channel_id: "checkout-team".to_string(),
                body: "브랜치를 준비합니다.".to_string(),
                fact_ids: BTreeSet::new(),
            },
            NpcActionDraft::CreateBranch {
                repository_id: "legacy-cart".to_string(),
                branch_name: "sim/minseo/retry".to_string(),
                purpose: "재시도 검증".to_string(),
            },
            NpcActionDraft::Escalate {
                target_actor_id: "chaewon".to_string(),
                summary: "브랜치 생성이 지연됩니다.".to_string(),
            },
        ],
    )
    .await;
    let context = context(BTreeMap::new());
    let log = CallLog::default();
    let (mut executor, _, _, _) = executor(&log, true);

    let failure = executor
        .execute_turn(&context, &turn, &ConversationSurface::DirectMessage)
        .await
        .unwrap_err();

    assert_eq!(failure.failed_action_index, Some(1));
    assert_eq!(failure.completed.len(), 1);
    assert!(matches!(
        failure.source,
        ExecutionError::Gateway(ref error)
            if error.kind == GatewayKind::GitHub && error.retryable
    ));
    assert_eq!(log.values(), vec!["buzz:channel", "github:create_branch"]);

    let resumed = executor
        .execute_turn(&context, &turn, &ConversationSurface::DirectMessage)
        .await
        .unwrap();

    assert_eq!(resumed.operations.len(), 3);
    assert!(resumed.operations[0].replayed);
    assert!(!resumed.operations[1].replayed);
    assert!(!resumed.operations[2].replayed);
    assert_eq!(
        log.values(),
        vec![
            "buzz:channel",
            "github:create_branch",
            "github:create_branch",
            "simulation:escalate",
        ]
    );
}

#[tokio::test]
async fn tampered_action_id_is_rejected_before_any_gateway_call() {
    let mut turn = validated_turn(
        None,
        vec![NpcActionDraft::SendMessage {
            channel_id: "checkout-team".to_string(),
            body: "정상 메시지".to_string(),
            fact_ids: BTreeSet::new(),
        }],
    )
    .await;
    turn.actions[0].action_id = "0".repeat(64);
    let context = context(BTreeMap::new());
    let log = CallLog::default();
    let (mut executor, _, _, _) = executor(&log, false);

    let failure = executor
        .execute_turn(&context, &turn, &ConversationSurface::DirectMessage)
        .await
        .unwrap_err();

    assert_eq!(failure.failed_action_index, Some(0));
    assert!(matches!(
        failure.source,
        ExecutionError::ActionIdMismatch { index: 0, .. }
    ));
    assert!(log.values().is_empty());
}

#[tokio::test]
async fn verification_must_match_current_head_and_trusted_manifest() {
    let wrong_head = "3333333333333333333333333333333333333333";
    let turn = validated_turn(
        None,
        vec![NpcActionDraft::RunVerification {
            repository_id: "legacy-cart".to_string(),
            commit_sha: wrong_head.to_string(),
            manifest_digest: MANIFEST_DIGEST.to_string(),
        }],
    )
    .await;
    let context = context(BTreeMap::new());
    let log = CallLog::default();
    let (mut executor, _, _, _) = executor(&log, false);

    let failure = executor
        .execute_turn(&context, &turn, &ConversationSurface::DirectMessage)
        .await
        .unwrap_err();

    assert!(matches!(
        failure.source,
        ExecutionError::HeadCommitMismatch { ref expected, ref actual, .. }
            if expected == HEAD_SHA && actual == wrong_head
    ));
    assert!(log.values().is_empty());

    let wrong_manifest = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let turn = validated_turn(
        None,
        vec![NpcActionDraft::RunVerification {
            repository_id: "legacy-cart".to_string(),
            commit_sha: HEAD_SHA.to_string(),
            manifest_digest: wrong_manifest.to_string(),
        }],
    )
    .await;
    let failure = executor
        .execute_turn(&context, &turn, &ConversationSurface::DirectMessage)
        .await
        .unwrap_err();

    assert!(matches!(
        failure.source,
        ExecutionError::ManifestDigestMismatch { ref expected, ref actual, .. }
            if expected == MANIFEST_DIGEST && actual == wrong_manifest
    ));
    assert!(log.values().is_empty());
}

#[tokio::test]
async fn request_review_requires_a_current_session_review_route() {
    let turn = validated_turn(
        None,
        vec![NpcActionDraft::RequestReview {
            repository_id: "legacy-cart".to_string(),
            pull_request: 7,
        }],
    )
    .await;
    let context = context(BTreeMap::new());
    let log = CallLog::default();
    let (mut executor, _, _, _) = executor(&log, false);

    let failure = executor
        .execute_turn(&context, &turn, &ConversationSurface::DirectMessage)
        .await
        .unwrap_err();

    assert!(matches!(
        failure.source,
        ExecutionError::NoReviewersConfigured { ref repository_id }
            if repository_id == "legacy-cart"
    ));
    assert!(log.values().is_empty());
}

fn _assert_executed_turn_is_send_sync(_: &ExecutedNpcTurn) {}
