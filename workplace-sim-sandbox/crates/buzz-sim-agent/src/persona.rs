use std::collections::{BTreeMap, BTreeSet};

use buzz_sim_github::RepositoryAccess;
use serde::{Deserialize, Serialize};

use crate::AgentError;

/// Current version of the workplace NPC persona pack schema.
pub const PERSONA_PACK_VERSION: u16 = 1;

/// Visual character presentation supported by the approved season design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterPresentation {
    /// A woman character. Season one intentionally contains only women NPCs.
    Woman,
}

/// Current scheduling state of an NPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NpcAvailability {
    /// The NPC may respond and propose work now.
    Available,
    /// The NPC is occupied but may still answer if the orchestrator elects to invoke her.
    Busy,
    /// The NPC must not be invoked for the current turn.
    Offline,
}

/// Work action categories an NPC is authorized to propose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NpcCapability {
    /// Send a message into an approved Buzz channel.
    SendMessage,
    /// Ask another actor to review work.
    RequestReview,
    /// Create a branch in a writable session repository.
    CreateBranch,
    /// Open a pull request from an existing branch.
    OpenPullRequest,
    /// Submit a pull-request review.
    ReviewPullRequest,
    /// Request objective sandbox verification for a commit.
    RunVerification,
    /// Escalate a risk or decision to another actor.
    Escalate,
    /// Schedule a bounded meeting with known actors.
    ScheduleMeeting,
}

/// Epistemic status of a persona knowledge entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeStance {
    /// The scenario defines this statement as a fact known to the NPC.
    Fact,
    /// The NPC currently believes the statement, but it may be wrong.
    Belief,
}

/// Maximum disclosure boundary for a knowledge entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeDisclosure {
    /// The fact may be stated in any approved surface.
    Public,
    /// The fact may be stated in a direct message or a channel subscribed by the NPC.
    Team,
    /// The fact may be disclosed only in a direct message.
    Discretionary,
    /// The fact is part of the NPC's private context and must never be emitted verbatim.
    Never,
}

/// One prioritized work objective that influences an NPC's judgment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpcGoal {
    /// Stable scenario-local goal identifier.
    pub id: String,
    /// Human-readable objective supplied to the model.
    pub description: String,
    /// Relative importance from zero through one hundred.
    pub priority: u8,
}

/// One fact or fallible belief available to an NPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEntry {
    /// Stable scenario-local fact identifier.
    pub id: String,
    /// Natural-language statement known or believed by the NPC.
    pub statement: String,
    /// Whether the statement is a fact or belief.
    pub stance: KnowledgeStance,
    /// Where the statement may be disclosed.
    pub disclosure: KnowledgeDisclosure,
}

/// Complete validated persona for one persistent workplace NPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpcPersona {
    /// Stable simulation actor identifier.
    pub id: String,
    /// Name shown in the visual-novel client.
    pub display_name: String,
    /// Character presentation; season one accepts only [`CharacterPresentation::Woman`].
    pub presentation: CharacterPresentation,
    /// Stable role identifier, such as `staff_backend_engineer`.
    pub role: String,
    /// Stable team identifier.
    pub team: String,
    /// Traits other characters can readily observe.
    pub public_traits: Vec<String>,
    /// Private motivations supplied only to this NPC's model context.
    pub private_traits: Vec<String>,
    /// Guidance for sentence length, tone, and uncertainty expression.
    pub speech_style: Vec<String>,
    /// Current prioritized work objectives.
    pub goals: Vec<NpcGoal>,
    /// Work actions this NPC may propose.
    pub capabilities: BTreeSet<NpcCapability>,
    /// Buzz channels this NPC may address directly.
    pub channels: BTreeSet<String>,
    /// Scenario repository access by logical repository identifier.
    pub repository_access: BTreeMap<String, RepositoryAccess>,
    /// Current workload from zero through one hundred.
    pub workload: u8,
    /// Current scheduling state.
    pub availability: NpcAvailability,
    /// Facts and beliefs available to this NPC.
    pub knowledge: Vec<KnowledgeEntry>,
}

impl NpcPersona {
    /// Finds one knowledge entry by stable fact identifier.
    #[must_use]
    pub fn knowledge_by_id(&self, fact_id: &str) -> Option<&KnowledgeEntry> {
        self.knowledge.iter().find(|entry| entry.id == fact_id)
    }
}

/// YAML-serializable collection of NPC personas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaPack {
    /// Persona schema version.
    pub version: u16,
    /// Persona definitions in authoring order.
    pub personas: Vec<NpcPersona>,
}

impl PersonaPack {
    /// Parses strict YAML while rejecting unknown fields and unsupported enum values.
    pub fn from_yaml(source: &str) -> Result<Self, AgentError> {
        serde_yaml::from_str(source).map_err(|error| AgentError::PersonaYaml(error.to_string()))
    }
}

/// Uniqueness-checked persona lookup table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaDirectory {
    personas: BTreeMap<String, NpcPersona>,
}

impl PersonaDirectory {
    /// Validates a persona pack and indexes personas by stable actor identifier.
    pub fn new(pack: PersonaPack) -> Result<Self, AgentError> {
        if pack.version != PERSONA_PACK_VERSION {
            return Err(AgentError::UnsupportedPersonaVersion(pack.version));
        }
        if pack.personas.is_empty() {
            return Err(AgentError::InvalidPersona {
                actor_id: "<pack>".to_string(),
                reason: "persona pack must contain at least one persona".to_string(),
            });
        }

        let mut personas = BTreeMap::new();
        for persona in pack.personas {
            validate_persona(&persona)?;
            let actor_id = persona.id.clone();
            if personas.insert(actor_id.clone(), persona).is_some() {
                return Err(AgentError::DuplicatePersonaId(actor_id));
            }
        }
        Ok(Self { personas })
    }

    /// Resolves a persona by its stable actor identifier.
    #[must_use]
    pub fn resolve(&self, actor_id: &str) -> Option<&NpcPersona> {
        self.personas.get(actor_id)
    }

    /// Iterates through personas in stable actor-ID order.
    pub fn personas(&self) -> impl Iterator<Item = &NpcPersona> {
        self.personas.values()
    }

    /// Returns the number of registered NPCs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.personas.len()
    }

    /// Returns whether no NPCs are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.personas.is_empty()
    }
}

fn validate_persona(persona: &NpcPersona) -> Result<(), AgentError> {
    validate_id(&persona.id).map_err(|reason| invalid_persona(persona, reason))?;
    validate_nonempty("display_name", &persona.display_name)
        .map_err(|reason| invalid_persona(persona, reason))?;
    validate_id(&persona.role).map_err(|reason| invalid_persona(persona, reason))?;
    validate_id(&persona.team).map_err(|reason| invalid_persona(persona, reason))?;
    if persona.workload > 100 {
        return Err(invalid_persona(
            persona,
            format!("workload {} exceeds 100", persona.workload),
        ));
    }

    let mut goal_ids = BTreeSet::new();
    for goal in &persona.goals {
        validate_id(&goal.id).map_err(|reason| invalid_persona(persona, reason))?;
        validate_nonempty("goal description", &goal.description)
            .map_err(|reason| invalid_persona(persona, reason))?;
        if goal.priority > 100 {
            return Err(invalid_persona(
                persona,
                format!("goal {:?} priority {} exceeds 100", goal.id, goal.priority),
            ));
        }
        if !goal_ids.insert(goal.id.clone()) {
            return Err(invalid_persona(
                persona,
                format!("duplicate goal id {:?}", goal.id),
            ));
        }
    }

    for channel_id in &persona.channels {
        validate_id(channel_id).map_err(|reason| invalid_persona(persona, reason))?;
    }
    for repository_id in persona.repository_access.keys() {
        validate_id(repository_id).map_err(|reason| invalid_persona(persona, reason))?;
    }

    let mut fact_ids = BTreeSet::new();
    for entry in &persona.knowledge {
        validate_id(&entry.id).map_err(|reason| invalid_persona(persona, reason))?;
        validate_nonempty("knowledge statement", &entry.statement)
            .map_err(|reason| invalid_persona(persona, reason))?;
        if !fact_ids.insert(entry.id.clone()) {
            return Err(AgentError::DuplicateKnowledgeId {
                actor_id: persona.id.clone(),
                fact_id: entry.id.clone(),
            });
        }
    }

    Ok(())
}

fn invalid_persona(persona: &NpcPersona, reason: String) -> AgentError {
    AgentError::InvalidPersona {
        actor_id: persona.id.clone(),
        reason,
    }
}

pub(crate) fn validate_id(value: &str) -> Result<(), String> {
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
        Err(format!(
            "invalid identifier {value:?}; expected ASCII letters, digits, '.', '_', ':', or '-'"
        ))
    }
}

pub(crate) fn validate_nonempty(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} must not be empty"))
    } else if value.len() > 8 * 1024 {
        Err(format!("{field} exceeds 8192 bytes"))
    } else {
        Ok(())
    }
}
