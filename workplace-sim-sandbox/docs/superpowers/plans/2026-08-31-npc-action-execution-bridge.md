# NPC Action Execution Bridge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert a policy-approved `ValidatedNpcTurn` into idempotent Buzz, GitHub, sandbox-verification, and simulation operations without allowing an LLM to mutate authoritative state directly.

**Architecture:** Add a `buzz-sim-executor` crate between `buzz-sim-agent` and external adapters. It revalidates the current persona and actor authority, verifies deterministic action IDs, resolves session repositories and review routes, executes operations sequentially, and stores successful receipts in a ledger so a retry resumes without duplicating completed work. Every gateway receives the deterministic operation ID and must implement idempotent semantics.

**Tech Stack:** Rust 1.88.0, async-trait, serde, serde_json, uuid, existing `buzz-sim-agent`, `buzz-sim-github`, and `buzz-sim-protocol` crates.

**Spec:** Approved conversation design for the 12-week workplace simulator and the completed NPC Persona & Orchestration phase.

## Global Constraints

- LLM output is never authoritative.
- All NPCs in season one remain women; no romance or affinity system is introduced.
- Repository, channel, actor, manifest, and commit authority are checked again at execution time.
- External side effects execute in deterministic order and stop on the first failure.
- Successful operations are replayed from a receipt ledger; failed operations may be retried with the same idempotency key.
- GitHub, build, test, and verification results are never synthesized by the executor.
- The exact final tree must pass format, check, test, Clippy with `-D warnings`, and diff hygiene in CI.

---

### Task 1: Define execution contracts and RED tests

**Files:**
- Create: `crates/buzz-sim-executor/Cargo.toml`
- Create: `crates/buzz-sim-executor/src/lib.rs`
- Create: `crates/buzz-sim-executor/tests/execution_bridge.rs`
- Modify: `Cargo.toml`

- [ ] Write tests for ordered dispatch, reply routing, repository resolution, review routing, exact verification binding, and typed receipts.
- [ ] Write tests for action-ID tampering, session mismatch, actor mismatch, commit mismatch, manifest mismatch, and missing review routes.
- [ ] Write tests proving a transient failure stops later actions and a retry reuses earlier successful receipts.
- [ ] Run CI and verify the tests fail because the execution API does not exist.

### Task 2: Implement execution context and command contracts

**Files:**
- Create: `crates/buzz-sim-executor/src/context.rs`
- Create: `crates/buzz-sim-executor/src/gateway.rs`
- Modify: `crates/buzz-sim-executor/src/lib.rs`

- [ ] Implement validated session repository targets, scenario identity, player identity, persona directory, GitHub actor directory, and default review routes.
- [ ] Define typed Buzz, GitHub, verification, and simulation commands and receipts.
- [ ] Define bounded, redacted gateway failures with explicit retryability.
- [ ] Run the targeted executor tests.

### Task 3: Implement idempotent ledger and deterministic identity checks

**Files:**
- Create: `crates/buzz-sim-executor/src/ledger.rs`
- Create: `crates/buzz-sim-executor/src/identity.rs`
- Modify: `crates/buzz-sim-executor/src/lib.rs`

- [ ] Recompute every action ID from session, turn, actor, index, and payload.
- [ ] Derive a deterministic reply operation ID and verification run UUID.
- [ ] Store command fingerprints and successful receipts in `MemoryExecutionLedger`.
- [ ] Reject one operation ID reused with a different resolved command.
- [ ] Run integrity and idempotency tests.

### Task 4: Implement sequential execution and partial resume

**Files:**
- Create: `crates/buzz-sim-executor/src/executor.rs`
- Modify: `crates/buzz-sim-executor/src/lib.rs`

- [ ] Revalidate the reply and every action against the current persona policy.
- [ ] Revalidate the NPC GitHub identity and repository access against `ActorDirectory`.
- [ ] Route the reply to the originating DM or channel.
- [ ] Resolve each action into an immutable gateway command.
- [ ] Execute reply first and actions in authoring order.
- [ ] Stop on first failure and return completed receipts plus failed action index.
- [ ] On retry, replay successful receipts and continue from the first incomplete operation.
- [ ] Run all executor tests.

### Task 5: Final quality gate

**Files:**
- Modify only files changed by `cargo fmt` or compiler-guided fixes.

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo check --workspace --all-targets`.
- [ ] Run `cargo test --workspace --all-targets`.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Run `git diff --check`.
- [ ] Trigger CI against the exact finalized commit and record the run result.
