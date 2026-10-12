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
  closed `<details>` with its count), plus each node's nested Done; `finished`
  is never collapsed or hidden (FR-019). `dismissed` appears only with
  `include_dismissed`. Project nodes are open by default and remember the
  viewer's folds (see the tree section).
- **Keyboard.** Plain links and buttons only, so Tab order is document order;
  a "skip to sessions" link first; visible `:focus-visible` ring (2px signal
  outline); section toggles are native `<details>`/`<summary>`; no focus
  traps, no hover-only controls. `j`/`k` are deliberately not added: native
  Tab and arrow behaviour stays predictable with a screen reader.
- **Motion.** One 150ms fade when a region is swapped by htmx, off under
  `prefers-reduced-motion`.

## The overview is a tree (one page for sessions and projects)

The overview and the old projects page were one idea split in two, so `/` is
now a tree: sessions nested under their projects, with a Move button on each.

- **Order.** The unchanged "Waiting on you" band first (cross-project; each
  row names its project, so nothing waiting hides inside a collapsed node),
  then one `<details open>` node per project (alphabetical), then a "No
  project" node (the global home of sessions without one; left out when
  empty), then the collapsed Unseen node and, with the toggle, Dismissed.
- **Node.** Header: project name (h2), status badge, state summary
  (`2 working · 1 interrupted · 3 finished`, from the sessions listed, so it
  follows the filters) and the plan figure when the project has a bound plan.
  Body: a quiet "Manage project" menu (rename, status, note, park or unpark,
  link to `/projects/{id}`), then Active, Interrupted, Finished and a nested
  Done. Finished is never collapsed; Done is a closed nested `<details>`.
  Waiting sessions appear in the band and in their node (band rows carry the
  `w-` id prefix, node rows `s-`). A project with no sessions stays visible,
  muted (`class="node empty"`), so a session can be dropped into it. A small
  "New empty project" form sits under the tree.
- **Move.** Each tree row has a Move button that reveals an inline panel (no
  modal): a labelled `<select>` of existing projects (the current one is
  selected and disabled), "No project (remove)" (`-`) and a last option "New
  project…" which reveals a name input. The panel posts to
  `POST /sessions/{id}/group`; the endpoint creates a missing project, so a
  new name and an existing name behave the same. `app.js` moves focus into
  the select, Escape or Cancel closes the panel and returns focus to the
  button, Move stays disabled until a real target (and, for a new project, a
  name) is chosen, and after a move focus lands on the session's Move button
  in its new place. Bulk moving is out of scope.
- **Folds are per viewer.** Node open/closed state lives in `localStorage`
  (`cc-nodes`, closed ids only, default open; every access in try/catch, with
  an in-memory fallback). It is written from the summary click, not from the
  `toggle` event, which also fires when a refresh swaps in new markup.
  `aria-expanded` on each node summary follows it.
- **Live refresh keeps your place.** `cc-changed` still re-fetches the whole
  `#sessions` region. Before the swap `app.js` records open Move panels, the
  values typed in open panels and open menus, the open menus and the focused
  element (by id); after the swap it re-applies the stored node folds and
  restores those. Only fields inside open panels or menus are restored, so a
  closed panel always shows what the server just rendered.
- **JSON.** `GET /` with `Accept: application/json` keeps the flat `sessions`
  list and adds `projects`: `[{id, name, status, summary, counts, progress?,
  sessions: [...]}]` (alphabetical, empty projects included, `progress` only
  when bound) and `no_project: {summary, counts, sessions}`. Every listed
  session appears once in the flat list and once in its node (sessions naming
  an unknown project fall under `no_project`); each carries its `section`, so
  a client can regroup. `GET /sessions` stays byte-for-byte `list --json`.

## Confirmation before destructive or hard-to-notice actions

`dismiss` (a session or a project), `unlink` and `Respawn` never act on the
first request in any interface (CLI prompt or `--yes`, TUI footer question,
and this view).

- **Dismiss and unlink.** `POST /sessions/{id}/dismiss`, `POST
  /projects/{id}/dismiss` and `POST /sessions/{id}/unlink` change nothing
  unless the body carries `confirm` (`1`, `true`, `on`, `yes`). Without it
  they answer 409 `{"ok":false,"error":...,"code":"confirmation_required"}`;
  an unknown target is still 404. The same path answers `GET` with the
  confirmation: JSON (`confirmation_required`, `action`, `consequence`,
  `confirm_with`) for scripts, an inline fragment for htmx requests, a whole
  page otherwise. `unlink` takes `?cortex=<node>`.
- **Panel, not modal.** The session page's Dismiss button `hx-get`s the
  fragment into `#confirm-slot`, above the page and outside the live region
  so a background refresh does not close it. It is a bordered panel in ink,
  not the signal colour: it names the target as a question, says the
  consequence, and offers Confirm (solid ink, a form that posts `confirm=1`)
  and Cancel (a plain link back to the page). `app.js` moves focus to
  Confirm, closes the panel on Escape or Cancel and returns focus to the
  button. The button sets `hx-select="unset"` because htmx would otherwise
  inherit the live region's filter and drop the fragment.
- **Respawn.** The Respawn button on the overview and the session page is a
  link to `/restore?session=<id>`: the `/restore` preview for that one
  session (any state, refusals and alternatives shown), whose Respawn button
  posts `/respawn` with that id. `POST /sessions/{id}/respawn` stays for
  scripts, as the contract says.
- **Not gated.** Reversible inverses (`undismiss`, `unpark`, `undone`) and
  notes, grouping, parking and filters. Setting a project's status to
  `archived` through the status form is `project set-status`, not `dismiss`,
  and is not gated.

## Pages

| Page | Route | Notes |
|---|---|---|
| Overview | `/`, `/sessions` | The project tree (below), Unseen (collapsed), Dismissed (with the toggle). Filter form (state, agent, repo, project, text, include dismissed) submits as a plain GET so it works without JS and applies to the tree. |
| Project | `/projects/{id}` | Deep link: header with name, status and notes, then the same node component for that one project. |
| Projects | `/projects` | 302 to `/` for pages; `Accept: application/json` still returns the project list. No nav entry. |
| Session | `/sessions/{id}` | Verbatim task and left-off, progress with source label, lineage (resumed from / by), notes, labelled generated summaries, actions. No transcript, no conversation text beyond what `show` prints. |
| Restore | `/restore`, `/restore?session={id}` | The interrupted list from `restore` without `--yes`, a checkbox per row (all selected), refusals listed with their alternatives, one confirm button posting to `/respawn`. With `session`, the preview of that one session, which is where the Respawn buttons lead. |
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
