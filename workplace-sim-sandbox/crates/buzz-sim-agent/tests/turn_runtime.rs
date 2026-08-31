use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use buzz_sim_agent::{
    AgentError, ConversationSurface, MemoryLedger, MemoryRecordOutcome, NpcActionCommand,
    NpcActionDraft, NpcActionExecutor, NpcActionExecutorError, NpcActionReceipt,
    NpcActionReceiptStatus, NpcModel, NpcModelInput, NpcModelOutput, NpcOrchestrator,
    NpcRuntimeError, NpcTurnRequest, NpcTurnRuntime, PersonaDirectory, PersonaPack, WorldSnapshot,
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
    private_traits: [레거시 시스템에 책임감을 느낌]
    speech_style: [짧은 문장]
    goals:
      - id: preserve_v1
        description: API v1을 보호한다
        priority: 90
    capabilities: [send_message, escalate]
    channels: [checkout-team]
    repository_access: {}
    workload: 10
    availability: available
    knowledge: []
"#;

type ModelCalls = Arc<Mutex<Vec<NpcModelInput>>>;
type ExecutorCalls = Arc<Mutex<Vec<NpcActionCommand>>>;
type TestRuntime = NpcTurnRuntime<CountingModel, PrefixExecutor>;

#[derive(Debug, Clone)]
struct CountingModel {
    calls: ModelCalls,
    output: NpcModelOutput,
}

#[async_trait]
impl NpcModel for CountingModel {
    async fn generate(&self, input: &NpcModelInput) -> Result<NpcModelOutput, AgentError> {
        self.calls.lock().unwrap().push(input.clone());
        Ok(self.output.clone())
    }
}

#[derive(Debug, Clone)]
struct PrefixExecutor {
    calls: ExecutorCalls,
    fail_on_call: Option<usize>,
}

#[async_trait]
impl NpcActionExecutor for PrefixExecutor {
    async fn execute(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError> {
        let call_index = {
            let mut calls = self.calls.lock().unwrap();
            let call_index = calls.len();
            calls.push(command.clone());
            call_index
        };
        if self.fail_on_call == Some(call_index) {
            return Err(NpcActionExecutorError::new("fake", "planned failure").unwrap());
        }
        NpcActionReceipt::new(
            command.action_id.clone(),
            NpcActionReceiptStatus::Completed,
            "fake",
            Some(format!("message:{}", command.sequence)),
            None,
        )
    }
}

fn request(player_input: &str) -> NpcTurnRequest {
    NpcTurnRequest {
        session_id: Uuid::parse_str("00000000-0000-4000-8000-000000000701").unwrap(),
        turn_id: Uuid::parse_str("00000000-0000-4000-8000-000000000702").unwrap(),
        actor_id: "minseo".to_string(),
        player_input: player_input.to_string(),
        surface: ConversationSurface::DirectMessage,
        world: WorldSnapshot {
            week: 3,
            sprint: 2,
            work_block: 84,
            active_incident: None,
            visible_facts: BTreeMap::new(),
        },
    }
}

fn message(body: &str) -> NpcActionDraft {
    NpcActionDraft::SendMessage {
        channel_id: "checkout-team".to_string(),
        body: body.to_string(),
        fact_ids: BTreeSet::new(),
    }
}

fn runtime(
    workload: u8,
    output: NpcModelOutput,
    fail_on_call: Option<usize>,
) -> (TestRuntime, ModelCalls, ExecutorCalls) {
    let personas = PERSONAS.replace("workload: 10", &format!("workload: {workload}"));
    let directory = PersonaDirectory::new(PersonaPack::from_yaml(&personas).unwrap()).unwrap();
    let model_calls = Arc::new(Mutex::new(Vec::new()));
    let executor_calls = Arc::new(Mutex::new(Vec::new()));
    let model = CountingModel {
        calls: model_calls.clone(),
        output,
    };
    let executor = PrefixExecutor {
        calls: executor_calls.clone(),
        fail_on_call,
    };
    let orchestrator = NpcOrchestrator::new(directory, MemoryLedger::default(), model);
    (
        NpcTurnRuntime::new(orchestrator, executor),
        model_calls,
        executor_calls,
    )
}

#[tokio::test]
async fn successful_turn_replay_does_not_repeat_model_or_side_effects_or_workload() {
    let output = NpcModelOutput {
        reply: None,
        actions: vec![message("상태를 공유합니다")],
        memory_note: Some("플레이어가 조기 공유를 요청했다".to_string()),
    };
    let (mut runtime, model_calls, executor_calls) = runtime(10, output, None);
    let request = request("팀 채널에 상태를 공유해 줘.");

    let first = runtime.run_turn(&request, 16, 1).await.unwrap();
    let second = runtime.run_turn(&request, 16, 1).await.unwrap();

    assert_eq!(model_calls.lock().unwrap().len(), 1);
    assert_eq!(executor_calls.lock().unwrap().len(), 1);
    assert_eq!(first.current_workload, 11);
    assert_eq!(second.current_workload, 11);
    assert_eq!(first.memory_outcome, Some(MemoryRecordOutcome::Inserted));
    assert_eq!(second.memory_outcome, Some(MemoryRecordOutcome::Duplicate));
    assert_eq!(second.dispatch.replayed_actions, 1);
    assert_eq!(runtime.orchestrator().memories().len(), 1);
}

#[tokio::test]
async fn changed_input_with_the_same_turn_id_is_rejected_before_model_or_executor() {
    let output = NpcModelOutput {
        reply: None,
        actions: vec![message("원래 메시지")],
        memory_note: None,
    };
    let (mut runtime, model_calls, executor_calls) = runtime(10, output, None);

    runtime
        .run_turn(&request("원래 요청"), 16, 1)
        .await
        .unwrap();
    let error = runtime
        .run_turn(&request("변조된 요청"), 16, 1)
        .await
        .unwrap_err();

    assert!(matches!(error, NpcRuntimeError::TurnReplayConflict { .. }));
    assert_eq!(model_calls.lock().unwrap().len(), 1);
    assert_eq!(executor_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn fully_exhausted_npc_is_rejected_before_the_model_is_called() {
    let output = NpcModelOutput {
        reply: None,
        actions: Vec::new(),
        memory_note: None,
    };
    let (mut runtime, model_calls, executor_calls) = runtime(100, output, None);

    let error = runtime
        .run_turn(&request("상태를 확인해 줘"), 16, 1)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        NpcRuntimeError::WorkloadExhausted {
            current: 100,
            required: 0,
            ..
        }
    ));
    assert!(model_calls.lock().unwrap().is_empty());
    assert!(executor_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn action_batch_over_budget_is_rejected_before_any_side_effect() {
    let output = NpcModelOutput {
        reply: None,
        actions: vec![message("하나"), message("둘"), message("셋")],
        memory_note: None,
    };
    let (mut runtime, model_calls, executor_calls) = runtime(98, output, None);

    let error = runtime
        .run_turn(&request("세 메시지를 보내 줘"), 16, 1)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        NpcRuntimeError::WorkloadExhausted {
            current: 98,
            required: 3,
            ..
        }
    ));
    assert_eq!(model_calls.lock().unwrap().len(), 1);
    assert!(executor_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn partial_failure_charges_only_the_completed_prefix_and_retry_finishes_without_regeneration()
{
    let output = NpcModelOutput {
        reply: None,
        actions: vec![message("하나"), message("둘"), message("셋")],
        memory_note: Some("세 단계 상태 공유를 맡았다".to_string()),
    };
    let (mut runtime, model_calls, executor_calls) = runtime(10, output, Some(1));
    let request = request("세 단계로 상태를 공유해 줘");

    let first_error = runtime.run_turn(&request, 16, 9).await.unwrap_err();
    assert!(matches!(first_error, NpcRuntimeError::Dispatch(_)));
    assert_eq!(runtime.current_workload(request.session_id, "minseo"), 11);
    assert!(runtime.orchestrator().memories().is_empty());

    let retry = runtime.run_turn(&request, 16, 9).await.unwrap();

    assert_eq!(model_calls.lock().unwrap().len(), 1);
    assert_eq!(executor_calls.lock().unwrap().len(), 4);
    assert_eq!(retry.dispatch.replayed_actions, 1);
    assert_eq!(retry.current_workload, 13);
    assert_eq!(retry.memory_outcome, Some(MemoryRecordOutcome::Inserted));
    assert_eq!(runtime.orchestrator().memories().len(), 1);
}
