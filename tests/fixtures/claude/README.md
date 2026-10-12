# Claude Code hook payload fixtures

These are real Claude Code 2.1.293 hook stdin payloads, captured 2026-10-12. Each file is one payload, redacted (home directory and scratch paths replaced with `/home/USER` and `/tmp/SCRATCH`) and truncated to 1500 characters per long text field.

## Naming

`<HookEvent>-<variant>.json`, where the variant is the event's discriminator:

- `Notification-<notification_type>.json` (`idle_prompt`, `permission_prompt`)
- `SessionStart-<source>.json` (`startup`, `resume`, `clear`)
- `SessionEnd-<reason>.json` (`other`, `clear`, `prompt_input_exit`)
- `PreToolUse-Read.json`, `PostToolUse-Read.json`, `PermissionRequest.json`, `Stop.json`, `UserPromptSubmit.json` (one per event)

## Caveats

- Long text fields are truncated, so their length does not match the real content. Do not assert on content length from these files.
- SessionStart `source` values in these fixtures: `startup`, `resume`, `clear`.
- SessionEnd `reason` values in these fixtures: `other`, `clear`, `prompt_input_exit`.
