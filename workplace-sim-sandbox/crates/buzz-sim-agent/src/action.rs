use std::collections::BTreeSet;

use buzz_sim_github::RepositoryAccess;
use serde::{Deserialize, Serialize};

use crate::{
    validate_id, validate_nonempty, ConversationSurface, KnowledgeDisclosure, NpcCapability,
    NpcPersona, PersonaDirectory,
};

/// Natural-language reply proposed by an NPC model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NpcReplyDraft {
    /// Reply body to display in the visual-novel or Buzz surface.
    pub body: String,
    /// Knowledge identifiers explicitly relied upon by the reply.
    pub fact_ids: BTreeSet<String>,
}

/// Structured, non-authoritative work action proposed by an NPC model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum NpcActionDraft {
    /// Post a message to an approved Buzz channel.
    SendMessage {
        /// Destination channel identifier.
        channel_id: String,
        /// Message body.
        body: String,
        /// Persona fact identifiers explicitly used by the message.
        fact_ids: BTreeSet<String>,
    },
    /// Create a work branch in one session repository.
    CreateBranch {
        /// Logical session repository identifier.
        repository_id: String,
        /// New branch name.
        branch_name: String,
        /// Short human-readable purpose.
        purpose: String,
    },
    /// Ask reviewers to inspect an existing pull request.
    RequestReview {
        /// Logical session repository identifier.
        repository_id: String,
        /// Positive pull-request number.
        pull_request: u64,
    },
    /// Open a pull request from a branch already created by an authorized actor.
    OpenPullRequest {
        /// Logical session repository identifier.
        repository_id: String,
        /// Source branch name.
        branch_name: String,
        /// Pull-request title.
        title: String,
        /// Pull-request description.
        body: String,
    },
    /// Submit review feedback to an existing pull request.
    ReviewPullRequest {
        /// Logical session repository identifier.
        repository_id: String,
        /// Positive pull-request number.
        pull_request: u64,
        /// Review body.
        body: String,
    },
    /// Request objective sandbox verification for one exact commit.
    RunVerification {
        /// Logical session repository identifier.
        repository_id: String,
        /// Full SHA-1 or SHA-256 Git object identifier.
        commit_sha: String,
        /// Lowercase or uppercase SHA-256 of the trusted verification manifest.
        manifest_digest: String,
    },
    /// Escalate a risk or decision to another known actor.
    Escalate {
        /// Stable target actor identifier.
        target_actor_id: String,
        /// Bounded escalation summary.
        summary: String,
    },
    /// Schedule a bounded meeting with known participants.
    ScheduleMeeting {
        /// Stable actor identifiers invited to the meeting.
        participant_actor_ids: BTreeSet<String>,
        /// Meeting agenda.
        agenda: String,
        /// Number of simulation work blocks consumed, from one through eight.
        duration_blocks: u8,
    },
}

impl NpcActionDraft {
    /// Returns the capability required before inspecting action-specific authority.
    #[must_use]
    pub const fn required_capability(&self) -> NpcCapability {
        match self {
            Self::SendMessage { .. } => NpcCapability::SendMessage,
            Self::CreateBranch { .. } => NpcCapability::CreateBranch,
            Self::RequestReview { .. } => NpcCapability::RequestReview,
            Self::OpenPullRequest { .. } => NpcCapability::OpenPullRequest,
            Self::ReviewPullRequest { .. } => NpcCapability::ReviewPullRequest,
            Self::RunVerification { .. } => NpcCapability::RunVerification,
            Self::Escalate { .. } => NpcCapability::Escalate,
            Self::ScheduleMeeting { .. } => NpcCapability::ScheduleMeeting,
        }
    }
}

/// Deterministic reason an NPC model proposal was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyViolation {
    /// Persona does not declare the required work capability.
    #[error("missing NPC capability {capability:?}")]
    CapabilityMissing {
        /// Missing capability.
        capability: NpcCapability,
    },
    /// Persona may inspect but not modify the named repository.
    #[error("repository write denied for {repository_id:?}")]
    RepositoryWriteDenied {
        /// Logical repository identifier.
        repository_id: String,
    },
    /// Persona has no access at all to the named repository.
    #[error("repository access denied for {repository_id:?}")]
    RepositoryAccessDenied {
        /// Logical repository identifier.
        repository_id: String,
    },
    /// Persona is not subscribed to the target Buzz channel.
    #[error("NPC is not subscribed to channel {channel_id:?}")]
    ChannelNotSubscribed {
        /// Rejected channel identifier.
        channel_id: String,
    },
    /// A cited knowledge entry cannot be disclosed on the current surface.
    #[error("fact disclosure denied for {fact_id:?}")]
    FactDisclosureDenied {
        /// Rejected fact identifier.
        fact_id: String,
    },
    /// Text contains the literal statement of a fact that cannot be disclosed.
    #[error("confidential text leak for {fact_id:?}")]
    ConfidentialTextLeak {
        /// Leaked fact identifier.
        fact_id: String,
    },
    /// A model cited a fact absent from the persona.
    #[error("unknown persona fact {fact_id:?}")]
    UnknownFact {
        /// Unknown fact identifier.
        fact_id: String,
    },
    /// A model addressed an actor absent from the persona directory.
    #[error("unknown target actor {actor_id:?}")]
    UnknownActor {
        /// Unknown actor identifier.
        actor_id: String,
    },
    /// A Git commit identifier is abbreviated or malformed.
    #[error("commit id must be a full 40-character SHA-1 or 64-character SHA-256")]
    InvalidCommitSha,
    /// A trusted manifest digest is not a 64-character SHA-256 value.
    #[error("manifest digest must be a 64-character SHA-256")]
    InvalidManifestDigest,
    /// A branch name is unsafe or malformed.
    #[error("invalid branch name {branch_name:?}")]
    InvalidBranchName {
        /// Rejected branch name.
        branch_name: String,
    },
    /// A pull-request number must be positive.
    #[error("pull-request number must be positive")]
    InvalidPullRequestNumber,
    /// A bounded text field is empty or oversized.
    #[error("invalid {field}: {reason}")]
    InvalidText {
        /// Stable field name.
        field: &'static str,
        /// Validation reason.
        reason: String,
    },
    /// A meeting has no participants or consumes an unsupported number of blocks.
    #[error("invalid meeting definition: {0}")]
    InvalidMeeting(String),
}

/// Validates non-authoritative model output against persona authority and disclosure boundaries.
#[derive(Debug, Clone, Copy)]
pub struct ActionPolicy<'a> {
    directory: &'a PersonaDirectory,
}

impl<'a> ActionPolicy<'a> {
    /// Creates a policy evaluator over the active persona directory.
    #[must_use]
    pub const fn new(directory: &'a PersonaDirectory) -> Self {
        Self { directory }
    }

    /// Validates one proposed work action without mutating the world.
    pub fn validate_action(
        &self,
        persona: &NpcPersona,
        action: &NpcActionDraft,
    ) -> Result<(), PolicyViolation> {
        self.require_capability(persona, action.required_capability())?;

        match action {
            NpcActionDraft::SendMessage {
                channel_id,
                body,
                fact_ids,
            } => {
                self.require_channel(persona, channel_id)?;
                let surface = ConversationSurface::Channel {
                    channel_id: channel_id.clone(),
                };
                self.validate_text_and_facts(persona, &surface, body, fact_ids, "message body")
            }
            NpcActionDraft::CreateBranch {
                repository_id,
                branch_name,
                purpose,
            } => {
                self.require_repository_write(persona, repository_id)?;
                if !valid_branch_name(branch_name) {
                    return Err(PolicyViolation::InvalidBranchName {
                        branch_name: branch_name.clone(),
                    });
                }
                self.validate_uncited_text(persona, purpose, "branch purpose")
            }
            NpcActionDraft::RequestReview {
                repository_id,
                pull_request,
            } => {
                self.require_repository_read(persona, repository_id)?;
                require_pull_request(*pull_request)
            }
            NpcActionDraft::OpenPullRequest {
                repository_id,
                branch_name,
                title,
                body,
            } => {
                self.require_repository_write(persona, repository_id)?;
                if !valid_branch_name(branch_name) {
                    return Err(PolicyViolation::InvalidBranchName {
                        branch_name: branch_name.clone(),
                    });
                }
                self.validate_uncited_text(persona, title, "pull-request title")?;
                self.validate_uncited_text(persona, body, "pull-request body")
            }
            NpcActionDraft::ReviewPullRequest {
                repository_id,
                pull_request,
                body,
            } => {
                self.require_repository_read(persona, repository_id)?;
                require_pull_request(*pull_request)?;
                self.validate_uncited_text(persona, body, "review body")
            }
            NpcActionDraft::RunVerification {
                repository_id,
                commit_sha,
                manifest_digest,
            } => {
                self.require_repository_read(persona, repository_id)?;
                if !full_git_object_id(commit_sha) {
                    return Err(PolicyViolation::InvalidCommitSha);
                }
                if !sha256_digest(manifest_digest) {
                    return Err(PolicyViolation::InvalidManifestDigest);
                }
                Ok(())
            }
            NpcActionDraft::Escalate {
                target_actor_id,
                summary,
            } => {
                validate_id(target_actor_id).map_err(|_| PolicyViolation::UnknownActor {
                    actor_id: target_actor_id.clone(),
                })?;
                if self.directory.resolve(target_actor_id).is_none() {
                    return Err(PolicyViolation::UnknownActor {
                        actor_id: target_actor_id.clone(),
                    });
                }
                self.validate_uncited_text(persona, summary, "escalation summary")
            }
            NpcActionDraft::ScheduleMeeting {
                participant_actor_ids,
                agenda,
                duration_blocks,
            } => {
                if participant_actor_ids.is_empty() || !(1..=8).contains(duration_blocks) {
                    return Err(PolicyViolation::InvalidMeeting(
                        "participants must be non-empty and duration must be 1..=8 blocks"
                            .to_string(),
                    ));
                }
                for actor_id in participant_actor_ids {
                    if self.directory.resolve(actor_id).is_none() {
                        return Err(PolicyViolation::UnknownActor {
                            actor_id: actor_id.clone(),
                        });
                    }
                }
                self.validate_uncited_text(persona, agenda, "meeting agenda")
            }
        }
    }

    /// Validates a proposed conversational reply on the current surface.
    pub fn validate_reply(
        &self,
        persona: &NpcPersona,
        surface: &ConversationSurface,
        reply: &NpcReplyDraft,
    ) -> Result<(), PolicyViolation> {
        if let ConversationSurface::Channel { channel_id } = surface {
            self.require_channel(persona, channel_id)?;
        }
        self.validate_text_and_facts(persona, surface, &reply.body, &reply.fact_ids, "reply body")
    }

    fn require_capability(
        &self,
        persona: &NpcPersona,
        capability: NpcCapability,
    ) -> Result<(), PolicyViolation> {
        if persona.capabilities.contains(&capability) {
            Ok(())
        } else {
            Err(PolicyViolation::CapabilityMissing { capability })
        }
    }

    fn require_channel(
        &self,
        persona: &NpcPersona,
        channel_id: &str,
    ) -> Result<(), PolicyViolation> {
        if persona.channels.contains(channel_id) {
            Ok(())
        } else {
            Err(PolicyViolation::ChannelNotSubscribed {
                channel_id: channel_id.to_string(),
            })
        }
    }

    fn require_repository_read(
        &self,
        persona: &NpcPersona,
        repository_id: &str,
    ) -> Result<RepositoryAccess, PolicyViolation> {
        persona
            .repository_access
            .get(repository_id)
            .copied()
            .ok_or_else(|| PolicyViolation::RepositoryAccessDenied {
                repository_id: repository_id.to_string(),
            })
    }

    fn require_repository_write(
        &self,
        persona: &NpcPersona,
        repository_id: &str,
    ) -> Result<(), PolicyViolation> {
        let access = self.require_repository_read(persona, repository_id)?;
        if access.can_write() {
            Ok(())
        } else {
            Err(PolicyViolation::RepositoryWriteDenied {
                repository_id: repository_id.to_string(),
            })
        }
    }

    fn validate_text_and_facts(
        &self,
        persona: &NpcPersona,
        surface: &ConversationSurface,
        body: &str,
        fact_ids: &BTreeSet<String>,
        field: &'static str,
    ) -> Result<(), PolicyViolation> {
        validate_text(field, body)?;
        for fact_id in fact_ids {
            let fact =
                persona
                    .knowledge_by_id(fact_id)
                    .ok_or_else(|| PolicyViolation::UnknownFact {
                        fact_id: fact_id.clone(),
                    })?;
            if !disclosure_allowed(fact.disclosure, surface) {
                return Err(PolicyViolation::FactDisclosureDenied {
                    fact_id: fact_id.clone(),
                });
            }
        }
        self.scan_literal_leaks(persona, surface, body)
    }

    fn validate_uncited_text(
        &self,
        persona: &NpcPersona,
        body: &str,
        field: &'static str,
    ) -> Result<(), PolicyViolation> {
        validate_text(field, body)?;
        self.scan_literal_leaks(persona, &ConversationSurface::DirectMessage, body)
    }

    fn scan_literal_leaks(
        &self,
        persona: &NpcPersona,
        surface: &ConversationSurface,
        body: &str,
    ) -> Result<(), PolicyViolation> {
        let normalized_body = body.to_lowercase();
        for fact in &persona.knowledge {
            if !disclosure_allowed(fact.disclosure, surface)
                && normalized_body.contains(&fact.statement.to_lowercase())
            {
                return Err(PolicyViolation::ConfidentialTextLeak {
                    fact_id: fact.id.clone(),
                });
            }
        }
        Ok(())
    }
}

fn disclosure_allowed(disclosure: KnowledgeDisclosure, surface: &ConversationSurface) -> bool {
    match (disclosure, surface) {
        (KnowledgeDisclosure::Public, _) => true,
        (KnowledgeDisclosure::Team, _) => true,
        (KnowledgeDisclosure::Discretionary, ConversationSurface::DirectMessage) => true,
        (KnowledgeDisclosure::Discretionary, ConversationSurface::Channel { .. })
        | (KnowledgeDisclosure::Never, _) => false,
    }
}

fn validate_text(field: &'static str, value: &str) -> Result<(), PolicyViolation> {
    validate_nonempty(field, value).map_err(|reason| PolicyViolation::InvalidText { field, reason })
}

fn require_pull_request(value: u64) -> Result<(), PolicyViolation> {
    if value == 0 {
        Err(PolicyViolation::InvalidPullRequestNumber)
    } else {
        Ok(())
    }
}

fn full_git_object_id(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64)
        && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn sha256_digest(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn valid_branch_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "@"
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.ends_with('.')
        && !value.ends_with(".lock")
        && !value.contains("..")
        && !value.contains("@{")
        && !value.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
}
