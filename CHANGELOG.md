# Changelog

## [0.2.0](https://github.com/alisonjenkins/claude-monitor/compare/v0.1.0...v0.2.0) (2026-09-27)


### Features

* add home-manager module registering the hook subcommand ([0baaa96](https://github.com/alisonjenkins/claude-monitor/commit/0baaa96d47ec449988d8922fe2d15d5be4e89456))
* **cli:** replace notify subcommand with a single hook entrypoint ([2f3c25b](https://github.com/alisonjenkins/claude-monitor/commit/2f3c25b92cfad5cc74578b610850baf5342dd5ca))
* **dashboard:** hide entries instead of deleting, coalesce watch events ([4c89c37](https://github.com/alisonjenkins/claude-monitor/commit/4c89c371f0b72a85a29b0031ca57a5561b94832b))
* **dashboard:** panic hook restores terminal, errors surface in footer ([ab094ce](https://github.com/alisonjenkins/claude-monitor/commit/ab094cea0f6fa6afcac369e76397866bd7552190))
* **flake:** add build/test/clippy/fmt checks and nixfmt formatter ([8d649ab](https://github.com/alisonjenkins/claude-monitor/commit/8d649ab69161e0ed184b716f8ca95a79d5629817))
* **hook:** treat PermissionRequest as needing permission ([4b64484](https://github.com/alisonjenkins/claude-monitor/commit/4b6448443d556a313ec2f31df823f976729fcf5d))
* initial implementation of claude-monitor TUI ([5455937](https://github.com/alisonjenkins/claude-monitor/commit/545593719d594029e9d8494ba9666a17f4e3a360))
* **notify:** notify only on transitions into an attention state ([d68b5fc](https://github.com/alisonjenkins/claude-monitor/commit/d68b5fc64f6568513d59c17d2473dca08ac2ac9f))
* **session:** model session state as an enum with atomic file IO ([f2aa319](https://github.com/alisonjenkins/claude-monitor/commit/f2aa319595502755d5047d0d936b3a4004b02b51))
* **tmux:** resolve pane locations via one cached list-panes call ([34d19fc](https://github.com/alisonjenkins/claude-monitor/commit/34d19fc179defd208b251bc9aa9c162dcb3941e8))
* **ui:** sort by priority, track selection by session id, render polish ([a044c60](https://github.com/alisonjenkins/claude-monitor/commit/a044c609a9da8b578fcce5e71165ae08f672fda4))


### Bug Fixes

* **dashboard:** ignore Ctrl/Alt chords for letter bindings ([2303411](https://github.com/alisonjenkins/claude-monitor/commit/2303411bd5b982341f9bdf309d919238b4034b6f))
* **flake:** replace removed darwin.apple_sdk.frameworks.Foundation ([6f82684](https://github.com/alisonjenkins/claude-monitor/commit/6f82684d272976f929e8aeef4e2e22b560409690))
* **tmux:** target panes by id and report failed switches ([3efae7d](https://github.com/alisonjenkins/claude-monitor/commit/3efae7d6c67803d84f2b0485efff0cae78938855))
* **ui:** describe the empty list as no sessions running ([4977c50](https://github.com/alisonjenkins/claude-monitor/commit/4977c50e1dc49225dbf68e661a5e79a965efcd48))
