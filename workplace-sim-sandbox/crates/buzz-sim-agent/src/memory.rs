use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{validate_id, validate_nonempty, AgentError, NpcPersona};

/// Visibility boundary attached to one remembered event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "audience", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryAudience {
    /// Visible only to one exact NPC.
    ActorOnly {
        /// Stable recipient actor identifier.
        actor_id: String,
    },
    /// Visible to NPCs assigned to one exact team.
    Team {
        /// Stable recipient team identifier.
        team_id: String,
    },
    /// Visible to every NPC in the simulation session.
    Public,
}

impl MemoryAudience {
    fn validate(&self) -> Result<(), AgentError> {
        match self {
            Self::ActorOnly { actor_id } => validate_id(actor_id)
                .map_err(|reason| AgentError::InvalidMemory(reason.to_string())),
            Self::Team { team_id } => {
                validate_id(team_id).map_err(|reason| AgentError::InvalidMemory(reason.to_string()))
            }
            Self::Public => Ok(()),
        }
    }

    fn visible_to(&self, persona: &NpcPersona) -> bool {
        match self {
            Self::ActorOnly { actor_id } => actor_id == &persona.id,
            Self::Team { team_id } => team_id == &persona.team,
            Self::Public => true,
        }
    }
}

/// Immutable memory derived from an authoritative simulation event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecord {
    /// Unique authoritative event identifier.
    pub event_id: Uuid,
    /// Simulation session that owns the event.
    pub session_id: Uuid,
    /// Monotonic session sequence used for deterministic ordering.
    pub sequence: u64,
    /// NPC that observed or recorded the event.
    pub actor_id: String,
    /// Visibility boundary applied when constructing model context.
    pub audience: MemoryAudience,
    /// Bounded natural-language summary.
    pub summary: String,
    /// Stable persona fact identifiers related to this memory.
    pub related_fact_ids: BTreeSet<String>,
}

impl MemoryRecord {
    /// Creates and validates an immutable memory record.
    pub fn new<I, S>(
        event_id: Uuid,
        session_id: Uuid,
        sequence: u64,
        actor_id: impl Into<String>,
        audience: MemoryAudience,
        summary: impl Into<String>,
        related_fact_ids: I,
    ) -> Result<Self, AgentError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let record = Self {
            event_id,
            session_id,
            sequence,
            actor_id: actor_id.into(),
            audience,
            summary: summary.into(),
            related_fact_ids: related_fact_ids.into_iter().map(Into::into).collect(),
        };
        validate_id(&record.actor_id)
            .map_err(|reason| AgentError::InvalidMemory(reason.to_string()))?;
        record.audience.validate()?;
        validate_nonempty("memory summary", &record.summary).map_err(AgentError::InvalidMemory)?;
        for fact_id in &record.related_fact_ids {
            validate_id(fact_id).map_err(|reason| AgentError::InvalidMemory(reason.to_string()))?;
        }
        Ok(record)
    }
}

/// Result of inserting one memory into the idempotent ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRecordOutcome {
    /// A previously unseen event was inserted.
    Inserted,
    /// The exact same event was replayed and no state changed.
    Duplicate,
}

/// In-memory projection of authoritative events available to NPC context builders.
#[derive(Debug, Clone, Default)]
pub struct MemoryLedger {
    records: BTreeMap<Uuid, MemoryRecord>,
}

impl MemoryLedger {
    /// Inserts one event idempotently and rejects conflicting reuse of its event ID.
    pub fn record(&mut self, record: MemoryRecord) -> Result<MemoryRecordOutcome, AgentError> {
        if let Some(existing) = self.records.get(&record.event_id) {
            return if existing == &record {
                Ok(MemoryRecordOutcome::Duplicate)
            } else {
                Err(AgentError::MemoryConflict(record.event_id))
            };
        }
        self.records.insert(record.event_id, record);
        Ok(MemoryRecordOutcome::Inserted)
    }

    /// Returns memories visible to one NPC in stable sequence order.
    #[must_use]
    pub fn visible_to(
        &self,
        session_id: Uuid,
        persona: &NpcPersona,
        limit: usize,
    ) -> Vec<MemoryRecord> {
        let mut visible = self
            .records
            .values()
            .filter(|record| record.session_id == session_id && record.audience.visible_to(persona))
            .cloned()
            .collect::<Vec<_>>();
        visible.sort_by_key(|record| (record.sequence, record.event_id));
        if visible.len() > limit {
            visible.drain(..visible.len() - limit);
        }
        visible
    }

    /// Returns the number of distinct authoritative events in the ledger.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Returns whether the ledger is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
