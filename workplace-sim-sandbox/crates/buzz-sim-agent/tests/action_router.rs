use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use buzz_sim_agent::{
    BuzzActionPort, GitHubActionPort, NpcActionCommand, NpcActionDraft, NpcActionExecutor,
    NpcActionExecutorError, NpcActionReceipt, NpcActionReceiptStatus, OrganizationActionPort,
    RoutedNpcActionExecutor, VerificationActionPort,
};
use uuid::Uuid;

type Calls = Arc<Mutex<Vec<String>>>;

#[derive(Debug, Clone)]
struct RecordingPort {
    category: &'static str,
    calls: Calls,
    fail: bool,
}

impl RecordingPort {
    fn new(category: &'static str, calls: Calls) -> Self {
        Self {
            category,
            calls,
            fail: false,
        }
    }

    fn failing(category: &'static str, calls: Calls) -> Self {
        Self {
            category,
            calls,
            fail: true,
        }
    }

    fn execute(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{}:{}", self.category, command.action_id));
        if self.fail {
            return Err(NpcActionExecutorError::new(self.category, "planned port failure").unwrap());
        }
        NpcActionReceipt::new(
            command.action_id.clone(),
            NpcActionReceiptStatus::Completed,
            self.category,
            Some(format!("{}:external", self.category)),
            None,
        )
    }
}

#[async_trait]
impl BuzzActionPort for RecordingPort {
    async fn execute_buzz_action(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError> {
        self.execute(command)
    }
}

#[async_trait]
impl GitHubActionPort for RecordingPort {
    async fn execute_github_action(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError> {
        self.execute(command)
    }
}

#[async_trait]
impl VerificationActionPort for RecordingPort {
    async fn execute_verification_action(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError> {
        self.execute(command)
    }
}

#[async_trait]
impl OrganizationActionPort for RecordingPort {
    async fn execute_organization_action(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError> {
        self.execute(command)
    }
}

fn command(index: u16, action: NpcActionDraft) -> NpcActionCommand {
    NpcActionCommand {
        session_id: Uuid::parse_str("00000000-0000-4000-8000-000000000801").unwrap(),
        turn_id: Uuid::parse_str("00000000-0000-4000-8000-000000000802").unwrap(),
        actor_id: "minseo".to_string(),
        action_id: format!("action-{index}"),
        sequence: index,
        input_digest: "1".repeat(64),
        output_digest: "2".repeat(64),
        action,
    }
}

fn router(calls: Calls) -> RoutedNpcActionExecutor<RecordingPort, RecordingPort, RecordingPort, RecordingPort> {
    RoutedNpcActionExecutor::new(
        RecordingPort::new("buzz", calls.clone()),
        RecordingPort::new("github", calls.clone()),
        RecordingPort::new("verification", calls.clone()),
        RecordingPort::new("organization", calls),
    )
}

#[tokio::test]
async fn routes_every_action_variant_to_exactly_one_backend_category() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let executor = router(calls.clone());
    let actions = vec![
        NpcActionDraft::SendMessage {
            channel_id: "checkout-team".to_string(),
            body: "진행 상황입니다".to_string(),
            fact_ids: BTreeSet::new(),
        },
        NpcActionDraft::CreateBranch {
            repository_id: "pricing-api".to_string(),
            branch_name: "npc/minseo/compat".to_string(),
            purpose: "API v1 호환성 보존".to_string(),
        },
        NpcActionDraft::RequestReview {
            repository_id: "pricing-api".to_string(),
            pull_request: 17,
        },
        NpcActionDraft::OpenPullRequest {
            repository_id: "pricing-api".to_string(),
            branch_name: "npc/minseo/compat".to_string(),
            title: "API v1 호환성 보존".to_string(),
            body: "계약 테스트를 추가합니다".to_string(),
        },
        NpcActionDraft::ReviewPullRequest {
            repository_id: "pricing-api".to_string(),
            pull_request: 17,
            body: "호환성 테스트가 필요합니다".to_string(),
        },
        NpcActionDraft::RunVerification {
            repository_id: "pricing-api".to_string(),
            commit_sha: "a".repeat(40),
            manifest_digest: "b".repeat(64),
        },
        NpcActionDraft::Escalate {
            target_actor_id: "chaewon".to_string(),
            summary: "금요일 출시 위험".to_string(),
        },
        NpcActionDraft::ScheduleMeeting {
            participant_actor_ids: BTreeSet::from(["seoyun".to_string()]),
            agenda: "출시 범위 조정".to_string(),
            duration_blocks: 1,
        },
    ];

    for (index, action) in actions.into_iter().enumerate() {
        let command = command(index as u16, action);
        let receipt = executor.execute(&command).await.unwrap();
        assert_eq!(receipt.action_id, command.action_id);
    }

    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            "buzz:action-0",
            "github:action-1",
            "github:action-2",
            "github:action-3",
            "github:action-4",
            "verification:action-5",
            "organization:action-6",
            "organization:action-7",
        ]
    );
}

#[tokio::test]
async fn backend_failure_is_returned_without_falling_through_to_another_port() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let executor = RoutedNpcActionExecutor::new(
        RecordingPort::new("buzz", calls.clone()),
        RecordingPort::failing("github", calls.clone()),
        RecordingPort::new("verification", calls.clone()),
        RecordingPort::new("organization", calls.clone()),
    );
    let command = command(
        0,
        NpcActionDraft::CreateBranch {
            repository_id: "pricing-api".to_string(),
            branch_name: "npc/minseo/fail".to_string(),
            purpose: "실패 테스트".to_string(),
        },
    );

    let error = executor.execute(&command).await.unwrap_err();

    assert!(error.to_string().contains("github executor failed"));
    assert_eq!(*calls.lock().unwrap(), vec!["github:action-0"]);
}
