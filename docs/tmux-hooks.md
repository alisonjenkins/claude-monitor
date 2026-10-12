# tmux hooks and the safety net

`programs.claude-monitor.tmuxHooks.enable` registers three tmux hooks that run
a reconcile, so a closed pane marks its session interrupted without waiting
for a poll. Verified on tmux 3.6a (research D12/R9).

| Event | Fires for | Command |
|---|---|---|
| `pane-exited` | the pane's process exits or is killed (exit, SIGTERM, SIGKILL) | `reconcile --pane <id>` |
| `pane-died` | the same, when `remain-on-exit` keeps the dead pane | `reconcile --pane <id>` |
| `window-unlinked` | a window goes; exposes only the window id, never a pane id | plain `reconcile` |

## The R9 gap

`kill-pane` of one pane in a multi-pane window fires no hook at all:
`pane-exited` does not run for it and `window-unlinked` does not run because
the window survives. Nothing runs when the tmux server itself dies either; the
login boot check covers that.

The 60 second `SafetyNetSource` tick covers the first case. Its refresh
reconciles every active session, and a session with no pid whose recorded pane
is no longer in `tmux list-panes` becomes interrupted. Worst case the session
shows as active for 60 seconds. `tests/safety_net.rs` covers this path.
