# Tasks: AI control centre

## Phase 1: Setup (shared)

- [X] T001 Create the `008-ai-control-centre` worktree and record the baseline
- [X] T002 [P] Draft ADR 0040 in `docs/adr/0040-store.md`
- [x] T006a [P] Spike R10: inspect `~/.claude/sessions/<pid>.json`

## Phase 2: Foundational (blocks all stories)

**Purpose**: core

- [X] T007 Refactor only: move `session` into the library
- [ ] T008 Add cargo features `tui` and `web`
- [ ] T009 [P] Add dependencies `rusqlite`

```markdown
- [ ] T999 this is an example inside a fence and must be ignored
```

## Phase 6: User Story 4 — where large projects stand (P2)

### Tests (write first)

- [ ] T083 [P] [US4] Parser tests incl. property tests
- [ ] T084 [P] [US4] Binding tests

### Implementation

- [ ] T086 [P] [US4] Implement the parsers in `src/progress/speckit.rs`
