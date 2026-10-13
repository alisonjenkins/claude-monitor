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
| CSRF | `app.js` adds `X-CC-Token` (read from `<meta name="cc-csrf">`) to every htmx request. The value is random per session and unrelated to the session cookie, which is `HttpOnly`; the page embeds the CSRF value for its own use | The guard requires the header on every POST (rule 4). Actions therefore need JavaScript; reads do not. |
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
| Session | `/sessions/{id}` | Verbatim task and left-off, progress with source label, lineage (resumed from / by), notes, labelled generated summaries, actions, and the stored tmux placement in the wording of `show` (or "no recorded tmux window"). Overview rows carry a muted `tmux session:window` hint when one is recorded (pages only; the list JSON keeps its shape). No transcript, no conversation text beyond what `show` prints. |
| Restore | `/restore`, `/restore?session={id}` | The interrupted list from `restore` without `--yes`, a checkbox per row (all selected), refusals listed with their alternatives, one confirm button posting to `/respawn`. With `session`, the preview of that one session, which is where the Respawn buttons lead. Below the list, a read-only "Live sessions — would restore as" panel (also `live` in the JSON) shows where each running session with a recorded tmux session would be put back, by the planner's target rules (override, then recorded session, then `agents`), and warns when two share a window and pane slot. It has no checkbox or button and does not change what `/respawn` starts. |
| Links | `/links` | Stored cortexiphan links; `?check=1` runs `links check` and marks disagreements. |

## Parity (SC-011)

`web::routes::COMMAND_ROUTES` maps a library command to the web route that
offers it; `tests/parity_web.rs` walks `Command::all()` and fails when a
command is neither mapped nor on the documented exclusion list, when a mapped
route does not exist, or when an excluded command has a TUI action.

Excluded, with reasons: `dashboard` (the terminal UI), `hook` (called by the
agents, never a person), `reconcile` (tmux hook entry point), `watch`
(long-running notifier), `boot-check` (login unit), `migrate` (one-off import),
`serve` (is the web view), `web` (CA, login and client certificate for it
from a terminal), `export-cortexiphan` (writes a file for another
system), `delete` (destructive, CLI with `--yes` only).

## Transport, login and the threat model

Decision record: encrypt and authenticate the browser connection, even on
loopback (2026-10-13).

**Why.** The view is powerful (it can respawn sessions and dismiss them) and
the first design guarded it with one long-lived 256-bit token that sat in a
URL, in a 0600 file and as the cookie value. Anything that could connect to the
port and learn that token had everything, and the token was readable in shell
history and the journal for as long as it lived.

**What was chosen.**

| Part | Choice | Reason |
|---|---|---|
| Transport | rustls 0.23 on the `ring` provider, TLS 1.3 only (the `tls12` feature is not compiled into the binary), HTTP/1.1 over ALPN, no plain-HTTP listener and no fallback | `ring` passes `cargo deny` and builds with a C compiler alone, so the nix build needs nothing new; `Cargo.lock` is read directly by the flake, so no hash entries change. aws-lc-rs would add a cmake build. |
| Certificates | `rcgen`: a per-install CA (5 years, name-constrained to `localhost`, `127.0.0.0/8`, `::1`) signs a 90-day leaf for `127.0.0.1`, `::1`, `localhost`; the leaf is re-issued at under 14 days left | Trusting the CA in a browser then cannot be used to impersonate any other site. Short leaves limit what a stolen leaf key is worth. |
| Files | `<state-dir>/tls/` mode 0700; `ca.pem`, `ca.key`, `server.pem`, `server.key` mode 0600 from creation (0600 staging file, then rename); keys are never printed or logged | The state directory is the trust boundary. |
| Login | A random 256-bit code in a printed URL: single use, 5 minutes, in memory only, compared in constant time | A URL that leaks later is already useless. The long-lived secret is gone. |
| Session | Random 256-bit id in a `__Host-`, `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/` cookie; server-side table, 8 h idle and 24 h absolute expiry, 64 entries; its own random CSRF value per session | The cookie is not the code and not anything on disk, so reading a file or a log never yields a live credential. A restart signs everyone out, which is the intended cost of keeping sessions off disk. |
| Throttle | 5 failed logins or unauthenticated requests per minute, globally, then 429 with `Retry-After`; a valid session is never throttled | On loopback every client looks alike, so the limit is global. It also refuses a valid code while locked out, so it is not an oracle. A local process can therefore lock new logins out for a minute; it can already stop the server. |
| Control socket | `<state-dir>/web.sock`, mode 0600, peer uid checked, one request (`login`) | New login URL without a restart and without a network listener. Bound in the 0700 `tls/` directory and renamed into place so it is never connectable by others. If the path is too long for a unix socket, `serve` runs without it and says so. |
| Mutual TLS | `serve --mtls` requires a client certificate in the handshake; `web client-cert` writes a PKCS#12 (`p12-keystore`, pure Rust, MIT OR Apache-2.0, AES-256 with HMAC-SHA256) | Refuses a connection before any HTTP is parsed. Off by default; login still applies. |
| One CA or two for mTLS | One CA. The server leaf carries the serverAuth usage and client certificates clientAuth, and rustls enforces the usage in each direction (tested: the server's own leaf is refused as a client certificate) | A second CA is a second key in the same directory and a second thing to import, with no extra isolation. |
| Headers | `Content-Security-Policy: default-src 'self'; style-src 'self' 'unsafe-inline'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'`, `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, `Cache-Control: no-store` | Inline styles stay allowed because progress bars set a width; no inline script ever runs. The browser e2e checks the page raises no policy violations. |

**Setting the browser up (`web setup-browser`).**

| Part | Choice | Reason |
|---|---|---|
| Tools | `certutil` and `pk12util` (NSS) from `PATH`, behind a small `CommandRunner` trait; stdin closed, 30 s timeout, output capped | The browsers' own certificate stores are NSS databases and these are the supported way to edit them. No new crate does it safely. Missing tools exit 2 with `nix-shell -p nss.tools`. |
| Targets | Firefox profiles from `profiles.ini` (`~/.mozilla/firefox`, `$XDG_CONFIG_HOME/mozilla/firefox`), Zen (`~/.zen`, `$XDG_CONFIG_HOME/zen`), Chromium-family `~/.pki/nssdb`, plus `--profile` | Read from `$HOME`; a browser that is not installed contributes nothing. Zen currently keeps its profiles under `~/.config/zen`. |
| Identity | `tls::ensure_client_identity` keeps `client.pem` / `client.key` in `tls/` and reuses them until 30 days from expiry or until the CA changes | `issue_client_identity` mints a new certificate every call, which would make every run a new import. |
| Idempotence | Compare SHA-256 fingerprints of what `certutil -L -n NICK -a` prints with ours; write only on a difference; replace a changed certificate by removing the nickname (and its key) first | A re-run changes and backs up nothing. pk12util would otherwise add `nick #2`. |
| Secrets | Bundle and its random password in 0600 files in `tls/` named by `-w`, removed by `Drop` on every exit path; `/dev/null` as the empty database password | Nothing sensitive in `argv`, where other users can read it. `/dev/null` also keeps a dry run from writing anything. |
| Safety | Refuse a profile whose `lock` names a live pid or whose `.parentlock` is held by another process; skip a primary-password database (detected by a key listing that fails with `SEC_ERROR_BAD_PASSWORD`); copy `cert9.db` and `key4.db` before the first write | A running browser holds the database and overwrites it on exit; certutil would prompt for a primary password. |
| Auto-select | Opt-in. `--auto-select` writes a marked block to `user.js` only; Chromium gets a printed managed policy | `security.default_personal_cert = "Select Automatically"` is global to the profile: every site asking for a client certificate gets this one. Chromium reads policy only from root-owned files. |

The limit stays the one above: the client key is stored in the browser profile,
so setting the browser up makes the connection easy, not safer against a process
running as your user.

Secrets use `zeroize` where they are held as strings or byte vectors that this
code owns (codes, session ids, CSRF values, key PEM and DER, the generated
password). Copies made by libraries (rustls, rcgen, the PKCS#12 writer, the
output text that prints the password) are not wiped.

**What this protects against.** A process or web page that can only reach the
port: it cannot log in without a code, cannot forge or guess a session,
cannot make the browser act for it (`SameSite=Strict`, CSRF header, `Host`
allow-list against DNS rebinding), and loopback traffic is encrypted. A client
without the install's certificate cannot even complete a handshake when
`--mtls` is on.

**What it does not protect against.**

- Anything running as your user. It can read `ca.key` and the state directory,
  ask the control socket for a login URL, read the printed URL from the
  terminal or the journal while the code is valid, or read the browser profile.
- Root, a debugger on the server process, or a hostile browser extension.
- The 5-minute window: whoever opens the printed URL first wins the code.
- A stale session in an open event stream: an `/events` stream that is already
  connected is not cut when its session expires; it ends when the browser
  closes it or the server stops.
- Trust you grant by importing the CA: it is name-constrained, but a browser
  that does not enforce name constraints would let the CA vouch for more.

## Safety notes

- Every write goes through the same library call the CLI uses, so refusals
  (`not_finished`, `worktree_missing`, ...) are identical. A respawn refusal
  is data in a 200 response, as the contract says; an organise refusal maps
  to 404 (unknown), 409 (precondition) or 400 (usage).
- Tests that start a session use a private tmux socket
  (`CLAUDE_MONITOR_TMUX_SOCKET` equivalent: an `Env` whose tmux is
  `SystemTmux::with_socket`), never the user's server.
