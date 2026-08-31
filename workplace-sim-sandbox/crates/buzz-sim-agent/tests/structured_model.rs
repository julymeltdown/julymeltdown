use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use buzz_sim_agent::{
    AgentError, ConversationSurface, JsonNpcModel, MemoryLedger, NpcContextBuilder, NpcModel,
    NpcModelRequest, NpcModelTransport, NpcTurnRequest, PersonaDirectory, PersonaPack,
    WorldSnapshot, MAX_ACTIONS_PER_TURN, NPC_MODEL_PROTOCOL_VERSION,
};
use uuid::Uuid;

const PERSONA: &str = r#"
version: 1
personas:
  - id: minseo
    display_name: 문민서
    presentation: woman
    role: staff_backend_engineer
    team: checkout
    public_traits: [무뚝뚝함]
    private_traits: [과거 장애를 숨기고 있음]
    speech_style: [짧은 문장]
    goals:
      - id: preserve_v1
        description: API v1을 보호한다
        priority: 90
    capabilities: [send_message, create_branch, run_verification]
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
"#;

#[derive(Debug, Clone)]
struct CapturingTransport {
    requests: Arc<Mutex<Vec<NpcModelRequest>>>,
    response: Vec<u8>,
}

#[async_trait]
impl NpcModelTransport for CapturingTransport {
    async fn complete(&self, request: &NpcModelRequest) -> Result<Vec<u8>, AgentError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(self.response.clone())
    }
}

fn input() -> buzz_sim_agent::NpcModelInput {
    let directory = PersonaDirectory::new(PersonaPack::from_yaml(PERSONA).unwrap()).unwrap();
    let request = NpcTurnRequest {
        session_id: Uuid::parse_str("00000000-0000-4000-8000-000000000501").unwrap(),
        turn_id: Uuid::parse_str("00000000-0000-4000-8000-000000000502").unwrap(),
        actor_id: "minseo".to_string(),
        player_input: "모바일 계약 위험부터 확인하고 작업 방향을 제안해 줘.".to_string(),
        surface: ConversationSurface::DirectMessage,
        world: WorldSnapshot {
            week: 3,
            sprint: 2,
            work_block: 81,
            active_incident: None,
            visible_facts: Default::default(),
        },
    };
    NpcContextBuilder::new(&directory, &MemoryLedger::default())
        .build(&request, 16)
        .unwrap()
}

#[tokio::test]
async fn structured_model_forwards_exact_context_and_parses_strict_json() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let transport = CapturingTransport {
        requests: requests.clone(),
        response: r#"{
          "reply": {
            "body": "모바일 앱이 v1을 사용합니다. 계약부터 확인하죠.",
            "fact_ids": ["mobile_v1"]
          },
          "actions": [],
          "memory_note": null
        }"#
        .as_bytes()
        .to_vec(),
    };
    let model = JsonNpcModel::new(transport);
    let input = input();

    let output = model.generate(&input).await.unwrap();

    assert_eq!(output.reply.unwrap().fact_ids.len(), 1);
    let request = &requests.lock().unwrap()[0];
    assert_eq!(request.version, NPC_MODEL_PROTOCOL_VERSION);
    assert_eq!(request.input.request.player_input, input.request.player_input);
    assert_eq!(request.input.persona.private_traits, input.persona.private_traits);
    assert_eq!(request.contract.maximum_actions, MAX_ACTIONS_PER_TURN);
    assert!(request.contract.strict_json_only);
    assert!(request
        .instructions
        .contains("Never invent tool, GitHub, build, test, or verification results"));
}

#[tokio::test]
async fn markdown_fenced_json_is_rejected_instead_of_heuristically_repaired() {
    let transport = CapturingTransport {
        requests: Arc::new(Mutex::new(Vec::new())),
        response: b"```json\n{\"reply\":null,\"actions\":[],\"memory_note\":null}\n```".to_vec(),
    };
    let model = JsonNpcModel::new(transport);

    let error = model.generate(&input()).await.unwrap_err();

    assert!(matches!(error, AgentError::Model(message) if message.contains("strict JSON")));
}

#[tokio::test]
async fn unknown_output_fields_are_rejected() {
    let transport = CapturingTransport {
        requests: Arc::new(Mutex::new(Vec::new())),
        response: br#"{
          "reply": null,
          "actions": [],
          "memory_note": null,
          "pretend_tests_passed": true
        }"#
        .to_vec(),
    };
    let model = JsonNpcModel::new(transport);

    let error = model.generate(&input()).await.unwrap_err();

    assert!(matches!(error, AgentError::Model(message) if message.contains("strict JSON")));
}

#[tokio::test]
async fn oversized_model_output_is_rejected_before_parsing() {
    let transport = CapturingTransport {
        requests: Arc::new(Mutex::new(Vec::new())),
        response: vec![b'x'; 1025],
    };
    let model = JsonNpcModel::with_response_byte_limit(transport, 1024).unwrap();

    let error = model.generate(&input()).await.unwrap_err();

    assert!(matches!(error, AgentError::Model(message) if message.contains("1025") && message.contains("1024")));
}
