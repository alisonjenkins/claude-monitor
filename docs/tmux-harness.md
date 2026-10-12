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
