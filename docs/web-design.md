# Web view design (T116, research D10)

The loopback view behind `claude-monitor serve`. Same data and actions as the
TUI (FR-030); this file records how it looks, how it is built and why.

## Templating and scripting

| Choice | Decision | Why |
|---|---|---|
| Templates | `maud`, compile-time HTML in Rust, behind the `web` feature only | Escaping is on by default, a typo is a compile error, no template files to ship or to keep in sync with the Nix build. MIT/Apache-2.0, already allowed by `deny.toml`. |
| Interactivity | `htmx` 2.0.4, one minified file vendored at `src/web/assets/htmx.min.js`, served from `/assets/htmx.min.js` | No JS build step (D10), no CDN (security rule 6). 0BSD; licence text and version in `src/web/assets/htmx.LICENSE`. |
| Live updates | One 20-line `app.js` opens `EventSource("/events")` and fires a `cc-changed` event on `body`; page regions carry `hx-get` plus `hx-trigger="cc-changed from:body delay:150ms"` and `hx-select`, so htmx re-fetches the same URL and swaps only the region | The server already pushes exactly-what-changed over `/events` (D12); the page just re-reads. No htmx SSE extension (a second vendored file) and no polling. |
| Actions | `hx-post` with `hx-swap="none"`; every POST answers JSON, and the response header `HX-Trigger: cc-changed` makes the regions refresh | One response shape for scripts and browser; failures (4xx) land in a `role="status"` flash line. |
| CSRF | `app.js` adds `X-CC-Token` (read from `<meta name="cc-csrf">`) to every htmx request. The cookie is `HttpOnly`, so the page embeds the same token for its own use | The guard requires the header on every POST (rule 4). Actions therefore need JavaScript; reads do not. |
| Fonts, icons, images | None fetched. System stacks only, glyphs are CSS shapes and Unicode | No external hosts, nothing to leak the page being open. |
| Bodies | POST bodies are JSON or `application/x-www-form-urlencoded` (htmx forms) | Axum `json` and `form` features; no hand-rolled parser. |

JSON and HTML share routes: `Accept: application/json` returns the JSON the
CLI prints for `list --json` and `show --json`, anything else the page.

## Aesthetic: a quiet ledger with one alarm

Purpose: glance at it between other work and know whether anything is waiting
on you. So the page is calm paper-and-ink, and exactly one thing is loud.

- **Waiting on you is the only loud thing.** Sessions in `needs_permission`
  or `interrupted` are lifted into a "Waiting on you" band at the top of the
  overview, above every section, in the signal colour (amber on light, hot
  amber on dark) with a thick left bar, a tinted ground, bold state word and
  the action button (Respawn / Open) in the row. A count in the page title
  (`(2) claude-monitor`) and a `restore` banner when interrupted sessions
  exist. Nothing else on the page uses the signal colour.
- **Everything else is muted.** Working, idle, finished and done rows use ink
  grey, 1px hairlines, no fills. State is a word plus a small shape (filled
  dot working, hollow idle, check finished), never colour alone.
- **Type.** Headings in a transitional serif stack (`"Iowan Old Style",
  "Palatino Linotype", Palatino, Georgia, serif`), data (ids, ages, figures,
  paths) in a monospace stack (`ui-monospace, "SF Mono", "Cascadia Code",
  Menlo, Consolas, monospace`), body in the system UI stack. All local fonts.
- **Colour.** CSS custom properties on `:root`: paper, ink, ink-muted, rule,
  signal, signal-ground, ok. A `prefers-color-scheme: dark` block swaps the
  values; no toggle, no script. Contrast checked at 4.5:1 for text in both.
- **Layout.** One column, max 68rem, fluid. Rows are a CSS grid: at 40rem and
  up `state | task + left-off | project/repo | age | actions`; below it the
  row stacks (state and age on one line, task below, actions below). No
  horizontal scroll at 320px; targets are at least 44px tall on touch.
- **Progress.** `4 of 10` in monospace with a thin bar; the source label sits
  directly after it (`tasks.md (spec-kit tasks)`, `agent todo list`), as in
  `show`. A stale source says so in words. Sessions with several candidate
  plans show "choose a plan", never a guessed figure (FR-016).
- **Collapsed sections as the TUI.** Only `unseen` is collapsed by default (a
  closed `<details>` with its count); `finished` is never collapsed or hidden
  (FR-019). `done` is shown, in the muted style, below finished. `dismissed`
  appears only with `include_dismissed`.
- **Keyboard.** Plain links and buttons only, so Tab order is document order;
  a "skip to sessions" link first; visible `:focus-visible` ring (2px signal
  outline); section toggles are native `<details>`/`<summary>`; no focus
  traps, no hover-only controls. `j`/`k` are deliberately not added: native
  Tab and arrow behaviour stays predictable with a screen reader.
- **Motion.** One 150ms fade when a region is swapped by htmx, off under
  `prefers-reduced-motion`.

## Pages

| Page | Route | Notes |
|---|---|---|
| Overview | `/`, `/sessions` | Waiting band, Active, Finished, Done, Unseen (collapsed). Filter form (state, agent, repo, project, text, include dismissed) submits as a plain GET so it works without JS. |
| Project | `/projects/{id}` | Header with name, status and notes; then Active, Interrupted, Finished and the project's own Done section. |
| Session | `/sessions/{id}` | Verbatim task and left-off, progress with source label, lineage (resumed from / by), notes, labelled generated summaries, actions. No transcript, no conversation text beyond what `show` prints. |
| Restore | `/restore` | The interrupted list from `restore` without `--yes`, a checkbox per row (all selected), refusals listed with their alternatives, one confirm button posting to `/respawn`. |
| Links | `/links` | Stored cortexiphan links; `?check=1` runs `links check` and marks disagreements. |

## Parity (SC-011)

`web::routes::COMMAND_ROUTES` maps a library command to the web route that
offers it; `tests/parity_web.rs` walks `Command::all()` and fails when a
command is neither mapped nor on the documented exclusion list, when a mapped
route does not exist, or when an excluded command has a TUI action.

Excluded, with reasons: `dashboard` (the terminal UI), `hook` (called by the
agents, never a person), `reconcile` (tmux hook entry point), `watch`
(long-running notifier), `boot-check` (login unit), `migrate` (one-off import),
`serve` (is the web view), `export-cortexiphan` (writes a file for another
system), `delete` (destructive, CLI with `--yes` only).

## Safety notes

- Every write goes through the same library call the CLI uses, so refusals
  (`not_finished`, `worktree_missing`, ...) are identical. A respawn refusal
  is data in a 200 response, as the contract says; an organise refusal maps
  to 404 (unknown), 409 (precondition) or 400 (usage).
- Tests that start a session use a private tmux socket
  (`CLAUDE_MONITOR_TMUX_SOCKET` equivalent: an `Env` whose tmux is
  `SystemTmux::with_socket`), never the user's server.
