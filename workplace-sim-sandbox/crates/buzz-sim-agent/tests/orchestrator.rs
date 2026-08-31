use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use buzz_sim_agent::{
    AgentError, ConversationSurface, MemoryLedger, NpcActionDraft, NpcModel, NpcModelInput,
    NpcModelOutput, NpcOrchestrator, NpcReplyDraft, NpcTurnRequest, PersonaDirectory, PersonaPack,
    PolicyViolation, WorldSnapshot,
};
use uuid::Uuid;

const PERSONAS: &str = r#"
version: 1
personas:
  - id: minseo
    display_name: 문민서
    presentation: woman
    role: staff_backend_engineer
    team: checkout
    public_traits: [무뚝뚝함]
    private_traits: [레거시 시스템에 강한 책임감을 느낌]
    speech_style: [짧은 문장]
    goals:
      - id: preserve_v1
        description: API v1을 보호한다
        priority: 90
    capabilities: [send_message, create_branch, request_review, run_verification, escalate]
    channels: [checkout-team, project-coupon]
    repository_access:
      legacy-cart: maintain
      mobile-contracts: read
    workload: 40
    availability: available
    knowledge:
      - id: mobile_v1
        statement: 모바일 앱은 API v1을 사용한다
        stance: fact
        disclosure: team
  - id: jisoo
    display_name: 차지수
    presentation: woman
    role: sre
    team: reliability
    public_traits: [피곤함]
    private_traits: [서비스 안정성을 가장 중요하게 생각함]
    speech_style: [짧고 건조한 문장]
    goals:
      - id: sleep
        description: 오늘은 장애 없이 퇴근한다
        priority: 80
    capabilities: [send_message, run_verification, escalate]
    channels: [incident-checkout]
    repository_access:
      legacy-cart: read
    workload: 100
    availability: offline
    knowledge: []
"#;

#[derive(Debug, Clone)]
struct CapturingModel {
    captured: Arc<Mutex<Vec<NpcModelInput>>>,
    output: NpcModelOutput,
}

#[async_trait]
impl NpcModel for CapturingModel {
    async fn generate(&self, input: &NpcModelInput) -> Result<NpcModelOutput, AgentError> {
        self.captured.lock().unwrap().push(input.clone());
        Ok(self.output.clone())
    }
}

fn request(actor_id: &str) -> NpcTurnRequest {
    NpcTurnRequest {
        session_id: Uuid::parse_str("00000000-0000-4000-8000-000000000401").unwrap(),
        turn_id: Uuid::parse_str("00000000-0000-4000-8000-000000000402").unwrap(),
        actor_id: actor_id.to_string(),
        player_input: "레거시 수정 브랜치를 만들고 모바일 계약 위험을 설명해 줘.".to_string(),
        surface: ConversationSurface::DirectMessage,
        world: WorldSnapshot {
            week: 3,
            sprint: 2,
            work_block: 87,
            active_incident: None,
            visible_facts: BTreeMap::from([(
                "ticket".to_string(),
                "쿠폰 금액 표시 오류".to_string(),
            )]),
        },
    }
}

#[tokio::test]
async fn free_text_is_passed_to_the_model_and_validated_actions_get_stable_ids() {
    let directory = PersonaDirectory::new(PersonaPack::from_yaml(PERSONAS).unwrap()).unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = CapturingModel {
        captured: captured.clone(),
        output: NpcModelOutput {
            reply: Some(NpcReplyDraft {
                body: "모바일 앱이 v1을 사용합니다. 먼저 계약을 유지해야 해요.".to_string(),
                fact_ids: BTreeSet::from(["mobile_v1".to_string()]),
            }),
            actions: vec![NpcActionDraft::CreateBranch {
                repository_id: "legacy-cart".to_string(),
                branch_name: "sim/minseo/coupon-fix".to_string(),
                purpose: "쿠폰 계산 수정".to_string(),
            }],
            memory_note: Some("플레이어가 계약 위험을 먼저 확인했다".to_string()),
        },
    };
    let orchestrator = NpcOrchestrator::new(directory, MemoryLedger::default(), model);
    let turn_request = request("minseo");

    let first = orchestrator.orchestrate(&turn_request, 16).await.unwrap();
    let second = orchestrator.orchestrate(&turn_request, 16).await.unwrap();

    assert_eq!(captured.lock().unwrap()[0].request.player_input, turn_request.player_input);
    assert_eq!(first.actor_id, "minseo");
    assert_eq!(first.session_id, turn_request.session_id);
    assert_eq!(first.turn_id, turn_request.turn_id);
    assert_eq!(first.actions.len(), 1);
    assert_eq!(first.actions[0].action, NpcActionDraft::CreateBranch {
        repository_id: "legacy-cart".to_string(),
        branch_name: "sim/minseo/coupon-fix".to_string(),
        purpose: "쿠폰 계산 수정".to_string(),
    });
    assert_eq!(first.input_digest, second.input_digest);
    assert_eq!(first.output_digest, second.output_digest);
    assert_eq!(first.actions[0].action_id, second.actions[0].action_id);
}

#[tokio::test]
async fn invalid_model_actions_are_rejected_before_any_world_mutation() {
    let directory = PersonaDirectory::new(PersonaPack::from_yaml(PERSONAS).unwrap()).unwrap();
    let model = CapturingModel {
        captured: Arc::new(Mutex::new(Vec::new())),
        output: NpcModelOutput {
            reply: None,
            actions: vec![NpcActionDraft::CreateBranch {
                repository_id: "mobile-contracts".to_string(),
                branch_name: "sim/minseo/change-contract".to_string(),
                purpose: "모바일 계약 변경".to_string(),
            }],
            memory_note: None,
        },
    };
    let orchestrator = NpcOrchestrator::new(directory, MemoryLedger::default(), model);

    let error = orchestrator
        .orchestrate(&request("minseo"), 16)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AgentError::ActionRejected {
            index: 0,
            violation: PolicyViolation::RepositoryWriteDenied { repository_id }
        } if repository_id == "mobile-contracts"
    ));
}

#[tokio::test]
async fn offline_npcs_are_rejected_before_the_model_is_called() {
    let directory = PersonaDirectory::new(PersonaPack::from_yaml(PERSONAS).unwrap()).unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = CapturingModel {
        captured: captured.clone(),
        output: NpcModelOutput {
            reply: None,
            actions: Vec::new(),
            memory_note: None,
        },
    };
    let orchestrator = NpcOrchestrator::new(directory, MemoryLedger::default(), model);

    let error = orchestrator
        .orchestrate(&request("jisoo"), 16)
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::NpcUnavailable { actor_id } if actor_id == "jisoo"));
    assert!(captured.lock().unwrap().is_empty());
}

#[tokio::test]
async fn model_output_is_bounded_to_prevent_unlimited_agent_fanout() {
    let directory = PersonaDirectory::new(PersonaPack::from_yaml(PERSONAS).unwrap()).unwrap();
    let actions = (0..9)
        .map(|index| NpcActionDraft::SendMessage {
            channel_id: "checkout-team".to_string(),
            body: format!("상태 {index}"),
            fact_ids: BTreeSet::new(),
        })
        .collect();
    let model = CapturingModel {
        captured: Arc::new(Mutex::new(Vec::new())),
        output: NpcModelOutput {
            reply: None,
            actions,
            memory_note: None,
        },
    };
    let orchestrator = NpcOrchestrator::new(directory, MemoryLedger::default(), model);

    let error = orchestrator
        .orchestrate(&request("minseo"), 16)
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::TooManyActions { count: 9, maximum: 8 }));
}
