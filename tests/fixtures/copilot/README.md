# Copilot CLI hook payload fixtures

Real hook stdin payloads, captured 2026-10-12 from GitHub Copilot CLI 1.0.93
(spikes R3 and R6). One file is one payload. Keys are sorted and output is
pretty-printed by `jq -S`; values are unchanged apart from the redaction below.

- `native/`: payloads from hooks registered in Copilot's own hooks file
  (camelCase). `native/resume-same/` and `native/resume-other/` are the runs
  that resumed a session (`source: "resume"`, same session id in both).
- `claude-style/`: payloads Copilot delivers to hooks read from a project
  `.claude/settings.json` (snake_case, `hook_event_name`, ISO-8601 string
  `timestamp`, `session_id`).

## Redaction

- Scratch directory prefix replaced by `/tmp/SCRATCH`.
- Home directory replaced by `/home/USER`.
- Any string longer than 1500 characters truncated (none was).
- No token-like strings were present; none were added.

## Observed values

- Every native payload has `sessionId`, `timestamp` (milliseconds, integer)
  and `cwd`. There is no pid field.
- Native payloads carry no event name, except `notification`
  (`hook_event_name: "Notification"`) and `permissionRequest`
  (`hookName: "permissionRequest"`). The event is otherwise implied by which
  handler ran, so a parser has to infer it from the fields present.
- `sessionStart`: `source` was `new` or `resume`; `initialPrompt`.
- `userPromptSubmitted`: `prompt`.
- `preToolUse`: `toolName`, `toolArgs`. `postToolUse` adds `toolResult`
  (`resultType`, `textResultForLlm`).
- `permissionRequest`: `hookName`, `toolName`, `toolInput`,
  `permissionSuggestions` (empty array).
- `notification`: `message`, `title`, `notification_type`. Only
  `permission_prompt` was observed; the docs also list `shell_completed`,
  `shell_detached_completed`, `agent_completed`, `agent_idle` and
  `elicitation_dialog`.
- `agentStop`: `transcriptPath` (the session's `events.jsonl`), `stopReason`
  (`end_turn`), `stop_hook_active`. It has no assistant text.
- `sessionEnd`: `reason` was `complete` or `user_exit`; the docs also list
  `error`, `abort` and `timeout`.

## Ordering

`sessionStart` is lazy: it fires on the first submitted prompt, not when the
CLI starts. In a `-p` run the order is `userPromptSubmitted`, `sessionStart`,
`agentStop`, `sessionEnd`. An event can therefore arrive for a session the
store has not seen a start for.
