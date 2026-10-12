# Unknown-state sessions in a real HOME

Source: `scripts/smoke-real.sh` against `/home/ali`, 2026-10-12T12:01:28Z.

| Agent | Sessions | `unknown` | Other states |
|---|---|---|---|
| claude | 138 | 126 | idle 9, working 3 |
| copilot | 39 | 39 | none |

(The task brief said 125 and 39; the live count moved by one while this ran.)

## What they are

- Sessions passive discovery found on disk (`~/.claude/projects/*/*.jsonl`,
  `~/.copilot/session-state`) that were never seen by a hook and have no live
  pid. `Session::new` leaves them `unknown` until a first hook arrives, so they
  carry only the `state_unknown` flag.
- Old. 138 of the 165 unknown sessions were last active before 2026-10-05;
  one dates from 2026-02-07. They are real, finished-looking conversations
  from before the hooks were installed (or from runs where hooks did not fire).
- Concentrated in a few cwds: 38 in `nix-config`, 38 in the
  `rustling-meandering-pie` worktree (all Copilot), the rest spread over
  many other worktrees. 84 of the 165 also carry `worktree_missing`: the
  worktree has since been deleted.
- Some are test clutter: 19 have a cwd under `/tmp/nix-shell.*/scratchpad`
  (smoke and probe runs), and about 25 Claude sessions have an empty or
  `<command-name>/clear` first prompt.
- The transcripts do not show an explicit end marker. The last record of 116
  of 126 sampled Claude transcripts is a `cost-state` metadata line, which is
  written regardless of whether the conversation ended. So the state cannot be
  inferred as finished from the transcript alone; `unknown` is the honest value.

## Consequence

They were all filed under Active, which buries the 12 sessions that really are
idle or working. They are not live, so they must not count as active.

## Change

`unknown` sessions get their own `unseen` section: collapsed by default (count
only), expandable (`list --expand-unseen`, `u` in the TUI), sorted by last
activity, never counted as active. They stay visible and selectable once
expanded, and the JSON lists all of them with `"section": "unseen"`.
