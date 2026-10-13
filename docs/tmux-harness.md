# tmux test harness

Spike R7: a private tmux server runs inside the Nix check sandbox. Proven with
`nix build .#checks.x86_64-linux.tmux-harness` (tmux 3.6a).

- `tests/tmux_harness.rs` starts `tmux -S <tempdir>/sock -f /dev/null` per
  test and runs `kill-server` on drop. It never reaches the user's server:
  `TMUX` and `TMUX_PANE` are removed and the socket is explicit.
- The sandbox has no `HOME` and no terminal. `TERM=dumb` plus an explicit `-S`
  socket is enough; no `TMUX_TMPDIR` is needed.
- Without tmux on `PATH` the tests skip with a message. The flake check sets
  `CLAUDE_MONITOR_REQUIRE_TMUX=1`, which turns that skip into a failure, so the
  check cannot pass vacuously.
- The plain `cargo-test` check has no tmux, so these tests skip there; the
  `tmux-harness` check is the one that runs them.

## Respawn placement

`restore` and `respawn` reopen a session as a window named after its recorded
tmux window, in its recorded tmux session, creating that session detached when
it is missing and closing the `claude-monitor-placeholder` window once an agent
window exists. The pane listing (`LIST_PANES_FORMAT`) is
`#{pane_id} #{window_index} #{pane_index} #{window_panes} #{window_layout}
#{window_name}:#{session_name}`; the layout has commas, braces and brackets but
no spaces, so it sits before the two names, which may hold spaces and so split
at the last `:`, which tmux never leaves in a session name. Windows open in
(session, recorded window index) order and are appended after the last window,
so the original order survives without forcing absolute indexes. Placement
(session, window name and index, pane index, pane count, layout) is recorded by
the hook (`tmux display-message -p -t $TMUX_PANE`) and by reconcile, and is
never cleared when a pane or tmux is gone. `CLAUDE_MONITOR_TMUX_SESSION` or
`config.json` `tmuxSession` overrides the session for everything.

### Split panes

Sessions that share (target session, recorded session, window index, window
name) form one window group, ordered by pane index. The first opens the window
(`new-window`); each further one runs `split-window -d -P -F '#{pane_id}' -t
<first pane> -c <cwd> -- argv`. Every pane keeps its own `respawn_request` row,
keyed by its own pane id. When the group opens as many panes as the recorded
`window_panes`, the recorded `window_layout` is applied with `select-layout`;
otherwise the panes are `tiled`. A layout failure is a note on the window's
last pane and never fails the respawn. If the first pane fails to start, the
others open windows of their own. A group of one, a session with no recorded
pane index, and panes recorded in different tmux sessions (possible only under
a session override) behave as plain windows. A window with many panes may hit
tmux's "no space for new pane" in a small detached session; that pane is
refused with the tmux message and can be retried.
