use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use buzz_sim_agent::{
    NpcActionCommand, NpcActionDispatcher, NpcActionDraft, NpcActionExecutor,
    NpcActionExecutorError, NpcActionReceipt, NpcActionReceiptStatus, NpcDispatchError,
    ValidatedNpcAction, ValidatedNpcTurn,
};
use uuid::Uuid;

#[derive(Debug, Default)]
struct ExecutorState {
    calls: Vec<NpcActionCommand>,
}

#[derive(Debug, Clone)]
struct FakeExecutor {
    state: Arc<Mutex<ExecutorState>>,
    fail_on_call: Option<usize>,
    mismatched_receipt: bool,
}

impl FakeExecutor {
    fn successful() -> Self {
        Self {
            state: Arc::new(Mutex::new(ExecutorState::default())),
            fail_on_call: None,
            mismatched_receipt: false,
        }
    }

    fn failing_on(call: usize) -> Self {
        Self {
            fail_on_call: Some(call),
            ..Self::successful()
        }
    }

    fn with_mismatched_receipt() -> Self {
        Self {
            mismatched_receipt: true,
            ..Self::successful()
        }
    }

    fn calls(&self) -> Vec<NpcActionCommand> {
        self.state.lock().unwrap().calls.clone()
    }
}

#[async_trait]
impl NpcActionExecutor for FakeExecutor {
    async fn execute(
        &self,
        command: &NpcActionCommand,
    ) -> Result<NpcActionReceipt, NpcActionExecutorError> {
        let call_index = {
            let mut state = self.state.lock().unwrap();
            let call_index = state.calls.len();
            state.calls.push(command.clone());
            call_index
        };

        if self.fail_on_call == Some(call_index) {
            return Err(NpcActionExecutorError::new("fake", "simulated executor failure").unwrap());
        }

        let action_id = if self.mismatched_receipt {
            "wrong-action-id".to_string()
        } else {
            command.action_id.clone()
        };
        NpcActionReceipt::new(
            action_id,
            NpcActionReceiptStatus::Completed,
            "fake",
            Some(format!("external:{}", command.sequence)),
            Some("a".repeat(64)),
        )
    }
}

fn action(id: &str, body: &str) -> ValidatedNpcAction {
    ValidatedNpcAction {
        action_id: id.to_string(),
        action: NpcActionDraft::SendMessage {
            channel_id: "checkout-team".to_string(),
            body: body.to_string(),
            fact_ids: BTreeSet::new(),
        },
    }
}

fn turn(actions: Vec<ValidatedNpcAction>) -> ValidatedNpcTurn {
    ValidatedNpcTurn {
        session_id: Uuid::parse_str("00000000-0000-4000-8000-000000000601").unwrap(),
        turn_id: Uuid::parse_str("00000000-0000-4000-8000-000000000602").unwrap(),
        actor_id: "minseo".to_string(),
        reply: None,
        actions,
        memory_note: None,
        input_digest: "1".repeat(64),
        output_digest: "2".repeat(64),
    }
}

#[tokio::test]
async fn dispatches_actions_in_declared_order_and_binds_turn_identity() {
    let executor = FakeExecutor::successful();
    let observer = executor.clone();
    let mut dispatcher = NpcActionDispatcher::new(executor);
    let validated = turn(vec![action("action-one", "첫 번째"), action("action-two", "두 번째")]);

    let result = dispatcher.dispatch(&validated).await.unwrap();
    let calls = observer.calls();

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].sequence, 0);
    assert_eq!(calls[1].sequence, 1);
    assert_eq!(calls[0].session_id, validated.session_id);
    assert_eq!(calls[0].turn_id, validated.turn_id);
    assert_eq!(calls[0].actor_id, "minseo");
    assert_eq!(calls[0].action_id, "action-one");
    assert_eq!(result.receipts.len(), 2);
    assert_eq!(result.replayed_actions, 0);
}

#[tokio::test]
async fn replaying_the_same_turn_returns_cached_receipts_without_reexecution() {
    let executor = FakeExecutor::successful();
    let observer = executor.clone();
    let mut dispatcher = NpcActionDispatcher::new(executor);
    let validated = turn(vec![action("action-one", "첫 번째"), action("action-two", "두 번째")]);

    let first = dispatcher.dispatch(&validated).await.unwrap();
    let second = dispatcher.dispatch(&validated).await.unwrap();

    assert_eq!(observer.calls().len(), 2);
    assert_eq!(first.receipts, second.receipts);
    assert_eq!(second.replayed_actions, 2);
    assert_eq!(dispatcher.ledger().len(), 2);
}

#[tokio::test]
async fn reusing_an_action_id_with_different_content_is_rejected() {
    let executor = FakeExecutor::successful();
    let observer = executor.clone();
    let mut dispatcher = NpcActionDispatcher::new(executor);

    dispatcher
        .dispatch(&turn(vec![action("same-action", "원래 내용")]))
        .await
        .unwrap();
    let error = dispatcher
        .dispatch(&turn(vec![action("same-action", "다른 내용")]))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        NpcDispatchError::ActionReplayConflict { action_id }
            if action_id == "same-action"
    ));
    assert_eq!(observer.calls().len(), 1);
}

#[tokio::test]
async fn executor_failure_stops_later_actions_and_keeps_prior_receipts() {
    let executor = FakeExecutor::failing_on(1);
    let observer = executor.clone();
    let mut dispatcher = NpcActionDispatcher::new(executor);
    let validated = turn(vec![
        action("action-one", "첫 번째"),
        action("action-two", "두 번째"),
        action("action-three", "세 번째"),
    ]);

    let error = dispatcher.dispatch(&validated).await.unwrap_err();

    assert!(matches!(
        error,
        NpcDispatchError::ExecutorFailed { action_id, .. }
            if action_id == "action-two"
    ));
    assert_eq!(
        observer
            .calls()
            .iter()
            .map(|command| command.action_id.as_str())
            .collect::<Vec<_>>(),
        vec!["action-one", "action-two"]
    );
    assert_eq!(dispatcher.ledger().len(), 1);
}

#[tokio::test]
async fn mismatched_executor_receipts_are_rejected_and_not_cached() {
    let executor = FakeExecutor::with_mismatched_receipt();
    let mut dispatcher = NpcActionDispatcher::new(executor);

    let error = dispatcher
        .dispatch(&turn(vec![action("expected-action", "메시지")]))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        NpcDispatchError::ReceiptActionMismatch { expected, actual }
            if expected == "expected-action" && actual == "wrong-action-id"
    ));
    assert!(dispatcher.ledger().is_empty());
}

#[tokio::test]
async fn duplicate_action_ids_in_one_turn_fail_before_side_effects() {
    let executor = FakeExecutor::successful();
    let observer = executor.clone();
    let mut dispatcher = NpcActionDispatcher::new(executor);

    let error = dispatcher
        .dispatch(&turn(vec![
            action("duplicate", "첫 번째"),
            action("duplicate", "두 번째"),
        ]))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        NpcDispatchError::DuplicateActionId { action_id }
            if action_id == "duplicate"
    ));
    assert!(observer.calls().is_empty());
    assert!(dispatcher.ledger().is_empty());
}
