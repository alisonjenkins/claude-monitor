---

description: "Task list for the resilient client (cortexiphan-resilient-client)"
---

# Tasks: Resilient Client (shared outbound-network wrapper)

**Tests**: Included — this repo follows TDD; every implementation task below pairs a failing test
with the code that makes it pass.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)

---

## Phase 1: Setup

- [X] T001 Add `cortexiphan-resilient-client` as a workspace member in `Cargo.toml` (repo root)
- [X] T002 Create the crate manifest `crates/cortexiphan-resilient-client/Cargo.toml` with
  dependencies (`tokio`, `rand`, `tracing`, `thiserror`, `bytes`) and optional features `reqwest`
  and `wasm`

**Checkpoint**: Crate exists and builds empty.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: The shared types and transport trait every user story's behavior is built on.

- [X] T003 [P] Define `HttpRequest`/`HttpResponse` and the `HttpTransport` trait in
  `crates/cortexiphan-resilient-client/src/transport/mod.rs`
- [X] T004 [P] Define `Idempotency` (`Unsafe`/`Safe`/`SafeWithKey`), `RetryPolicy`, and `Attempt` in
  `crates/cortexiphan-resilient-client/src/lib.rs`
- [X] T005 [P] Define `ResilientClientError` and `TransportError` in
  `crates/cortexiphan-resilient-client/src/error.rs`
- [X] T006 Unit tests for `RetryPolicy` validation in `crates/cortexiphan-resilient-client/src/lib.rs`
- [X] T010 [P] Compile-only test: `GlooTransport` builds for `wasm32-unknown-unknown` — CI check,
  not a `cargo test`

**Checkpoint**: Foundation ready — user story implementation can now begin in parallel.

---

## Phase 3: User Story 1 - A brief external hiccup does not become a visible failure (Priority: P1) 🎯 MVP

**Goal**: Retry a transient failure with exponential backoff and jitter.

- [X] T011 [P] [US1] Test: a transient failure is retried until it succeeds
- [ ] T012 [US1] Implement the retry loop in `crates/cortexiphan-resilient-client/src/lib.rs`
- [ ] T013 [P] [US1] Test: a permanent failure is not retried
- [ ] T014 [US1] Wire the GitHub client through the resilient client

**Checkpoint**: A brief hiccup no longer surfaces to the caller.
