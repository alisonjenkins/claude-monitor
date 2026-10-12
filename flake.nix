{
  description = "Claude Code session monitor - TUI dashboard for managing multiple Claude sessions in tmux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Not pinned to nixpkgs.follows: programs.claude-code is new enough that
    # it only exists on home-manager's unstable branch, which in turn needs a
    # newer nixpkgs than this repo's own pin. Only the hm-module eval check
    # uses this input, via home-manager's own pkgs — it never reaches the
    # package or devShell outputs.
    home-manager.url = "github:nix-community/home-manager";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      home-manager,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

      # Shared native/build inputs for the package and every cargo-based check
      # below, so the darwin/linux split is defined in exactly one place.
      rustEnvFor = pkgs: {
        nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.pkg-config ];
        buildInputs =
          pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.dbus ]
          ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.apple-sdk ];
      };
      cargoBaseArgs = {
        pname = "claude-monitor";
        # release-please bumps Cargo.toml; read it so the flake never drifts.
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
      };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.rustPlatform.buildRustPackage (
            cargoBaseArgs
            // rustEnvFor pkgs
            // {
              meta.license = with pkgs.lib.licenses; [
                mit
                asl20
              ];
            }
          );
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          rust = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
            ];
          };
        in
        {
          default = pkgs.mkShell {
            buildInputs = [
              rust
              pkgs.cargo-watch
              pkgs.cargo-deny
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
              pkgs.pkg-config
              pkgs.dbus
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.apple-sdk ];
          };
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          rustEnv = rustEnvFor pkgs;
          # cargo's vendor dir is set up by cargoSetupHook regardless of the
          # phase contents, so overriding build/install to no-ops and putting
          # the real work in checkPhase still runs fully offline and cached.
          mkCargoCheck =
            {
              name,
              extraNativeBuildInputs ? [ ],
              command,
            }:
            pkgs.rustPlatform.buildRustPackage (
              cargoBaseArgs
              // rustEnv
              // {
                pname = "claude-monitor-${name}";
                nativeBuildInputs = rustEnv.nativeBuildInputs ++ extraNativeBuildInputs;
                doCheck = true;
                buildPhase = "true";
                installPhase = "touch $out";
                checkPhase = ''
                  runHook preCheck
                  ${command}
                  runHook postCheck
                '';
              }
            );
        in
        {
          package = self.packages.${system}.default;

          cargo-test = mkCargoCheck {
            name = "test";
            command = "cargo test --offline --locked";
          };

          clippy = mkCargoCheck {
            name = "clippy";
            extraNativeBuildInputs = [ pkgs.clippy ];
            command = "cargo clippy --offline --locked --all-targets -- -D warnings";
          };

          fmt = mkCargoCheck {
            name = "fmt";
            extraNativeBuildInputs = [ pkgs.rustfmt ];
            command = "cargo fmt --all -- --check";
          };

          # Evaluates the home-manager module against a minimal configuration
          # and asserts every required hook event is registered. Pure
          # evaluation (no build, no IFD): the assert is forced by nix flake
          # check when it tries to build this attribute, so an eval failure
          # here still surfaces as a normal check failure.
          hm-module =
            let
              hmPkgs = import home-manager.inputs.nixpkgs { inherit system; };
              mkHmConfig =
                copilot:
                mkHmConfigWith {
                  inherit copilot;
                  tmuxHooks = false;
                };
              mkHmConfigWith =
                {
                  copilot,
                  tmuxHooks,
                  watch ? false,
                  bootCheck ? false,
                  bootCheckBanner ? false,
                  hostSystem ? system,
                }:
                home-manager.lib.homeManagerConfiguration {
                  pkgs = if hostSystem == system then hmPkgs else import home-manager.inputs.nixpkgs { system = hostSystem; };
                  modules = [
                    self.homeManagerModules.default
                    {
                      home.username = "claude-monitor-check";
                      home.homeDirectory = "/home/claude-monitor-check";
                      home.stateVersion = "24.05";
                      programs.claude-monitor.enable = true;
                      programs.claude-monitor.copilot.enable = copilot;
                      programs.claude-monitor.tmuxHooks.enable = tmuxHooks;
                      programs.claude-monitor.watch.enable = watch;
                      programs.claude-monitor.bootCheck.enable = bootCheck;
                      programs.claude-monitor.bootCheck.banner.enable = bootCheckBanner;
                      programs.tmux.enable = true;
                    }
                  ];
                };
              hmConfig = mkHmConfig false;
              hooks = hmConfig.config.programs.claude-code.settings.hooks;

              # Copilot hooks file: absent by default, and when enabled it must
              # be valid JSON v1 with exactly the eight camelCase events, each
              # running the store-path binary (no PATH reliance).
              copilotFile = ".copilot/hooks/claude-monitor.json";
              hasCopilotFile = cfg: builtins.hasAttr copilotFile cfg.config.home.file;
              # fromJSON refuses strings that carry store-path context (the
              # handler commands reference the package), so drop it for parsing.
              copilotParsed = builtins.fromJSON (
                builtins.unsafeDiscardStringContext (mkHmConfig true).config.home.file.${copilotFile}.text
              );
              expectedCopilotEvents = [
                "agentStop"
                "notification"
                "permissionRequest"
                "postToolUse"
                "preToolUse"
                "sessionEnd"
                "sessionStart"
                "userPromptSubmitted"
              ];
              copilotHandlerOk =
                event: handlers:
                builtins.length handlers == 1
                && builtins.all (
                  h:
                  h.type == "command"
                  && h.timeoutSec == 10
                  && pkgs.lib.hasPrefix "/nix/store/" h.bash
                  && pkgs.lib.hasInfix "hook --agent copilot" h.bash
                  && pkgs.lib.hasSuffix " --event ${event}" h.bash
                ) handlers;
              copilotProblems =
                pkgs.lib.optional (hasCopilotFile hmConfig) "copilot hooks file generated with copilot.enable = false"
                ++ pkgs.lib.optional (
                  !hasCopilotFile (mkHmConfig true)
                ) "copilot hooks file missing with copilot.enable = true"
                ++ pkgs.lib.optional (copilotParsed.version != 1) "copilot hooks file version is not 1"
                ++ pkgs.lib.optional (
                  builtins.sort builtins.lessThan (builtins.attrNames copilotParsed.hooks) != expectedCopilotEvents
                ) "copilot hooks file events differ from the expected eight"
                ++ pkgs.lib.optional (
                  !builtins.all (e: copilotHandlerOk e copilotParsed.hooks.${e}) (builtins.attrNames copilotParsed.hooks)
                ) "a copilot handler is not a store-path `hook --agent copilot --event <its event>` command with timeoutSec 10";
              # tmux hooks: absent by default; when enabled the generated
              # tmux config must carry the three hooks, each running the
              # store-path binary, and the pane-scoped ones pass the pane id.
              tmuxConf = cfg: builtins.unsafeDiscardStringContext cfg.config.xdg.configFile."tmux/tmux.conf".text;
              tmuxOn = tmuxConf (mkHmConfigWith {
                copilot = false;
                tmuxHooks = true;
              });
              tmuxOff = tmuxConf hmConfig;
              hookLine =
                conf: hook:
                builtins.filter (l: pkgs.lib.hasPrefix "set-hook -g ${hook} " l) (pkgs.lib.splitString "\n" conf);
              hookOk =
                hook: needle:
                let
                  ls = hookLine tmuxOn hook;
                in
                builtins.length ls == 1
                && pkgs.lib.hasInfix "/nix/store/" (builtins.head ls)
                && pkgs.lib.hasInfix needle (builtins.head ls);
              tmuxProblems =
                builtins.concatMap (
                  h:
                  pkgs.lib.optional (hookLine tmuxOff h != [ ]) "tmux hook ${h} present with tmuxHooks.enable = false"
                ) tmuxHookNames
                ++ pkgs.lib.optional (
                  !hookOk "pane-exited" "reconcile --pane #{hook_pane}"
                ) "tmux pane-exited hook missing or not a store-path `reconcile --pane`"
                ++ pkgs.lib.optional (
                  !hookOk "pane-died" "reconcile --pane #{hook_pane}"
                ) "tmux pane-died hook missing or not a store-path `reconcile --pane`"
                ++ pkgs.lib.optional (
                  !hookOk "window-unlinked" "claude-monitor reconcile\""
                ) "tmux window-unlinked hook missing or not a store-path `reconcile`";
              tmuxHookNames = [
                "pane-exited"
                "pane-died"
                "window-unlinked"
              ];
              # watch service: absent by default; when enabled on Linux it is a
              # user service that restarts on failure, runs the store-path
              # binary's `watch` and starts with the session.
              watchServiceName = "claude-monitor-watch";
              hasWatchService = cfg: builtins.hasAttr watchServiceName cfg.config.systemd.user.services;
              watchOn =
                (mkHmConfigWith {
                  copilot = false;
                  tmuxHooks = false;
                  watch = true;
                }).config.systemd.user.services.${watchServiceName};
              watchProblems =
                pkgs.lib.optional (hasWatchService hmConfig) "watch service generated with watch.enable = false"
                ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
                  let
                    exec = builtins.unsafeDiscardStringContext (
                      builtins.head (pkgs.lib.toList watchOn.Service.ExecStart)
                    );
                  in
                  pkgs.lib.optional (
                    !(pkgs.lib.hasPrefix "/nix/store/" exec && pkgs.lib.hasSuffix "/bin/claude-monitor watch" exec)
                  ) "watch service ExecStart is not the store-path `claude-monitor watch`"
                  ++ pkgs.lib.optional (
                    watchOn.Service.Restart != "on-failure"
                  ) "watch service does not restart on failure"
                  ++ pkgs.lib.optional (
                    watchOn.Install.WantedBy != [ "default.target" ]
                  ) "watch service is not wanted by default.target"
                );
              # boot-check unit: absent by default; when enabled on Linux it is a
              # oneshot user unit running the store-path binary's `boot-check`
              # at login.
              bootCheckServiceName = "claude-monitor-boot-check";
              hasBootCheckService = cfg: builtins.hasAttr bootCheckServiceName cfg.config.systemd.user.services;
              bootCheckOn =
                (mkHmConfigWith {
                  copilot = false;
                  tmuxHooks = false;
                  bootCheck = true;
                }).config.systemd.user.services.${bootCheckServiceName};
              bootCheckProblems =
                pkgs.lib.optional (
                  hasBootCheckService hmConfig
                ) "boot-check unit generated with bootCheck.enable = false"
                ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
                  let
                    exec = builtins.unsafeDiscardStringContext (
                      builtins.head (pkgs.lib.toList bootCheckOn.Service.ExecStart)
                    );
                  in
                  pkgs.lib.optional (
                    !(pkgs.lib.hasPrefix "/nix/store/" exec && pkgs.lib.hasSuffix "/bin/claude-monitor boot-check" exec)
                  ) "boot-check unit ExecStart is not the store-path `claude-monitor boot-check`"
                  ++ pkgs.lib.optional (
                    bootCheckOn.Service.Type != "oneshot"
                  ) "boot-check unit is not a oneshot"
                  ++ pkgs.lib.optional (
                    bootCheckOn.Install.WantedBy != [ "default.target" ]
                  ) "boot-check unit is not wanted by default.target"
                );
              # launchd agent: evaluated against a darwin host from any system
              # (nothing is built); it must run the store-path `boot-check`
              # once at load and must not appear on Linux.
              bootCheckDarwin =
                (mkHmConfigWith {
                  copilot = false;
                  tmuxHooks = false;
                  bootCheck = true;
                  hostSystem = "aarch64-darwin";
                }).config.launchd.agents.${bootCheckServiceName};
              darwinArgs = map builtins.unsafeDiscardStringContext bootCheckDarwin.config.ProgramArguments;
              launchdProblems =
                pkgs.lib.optional (
                  !(
                    builtins.length darwinArgs == 2
                    && pkgs.lib.hasPrefix "/nix/store/" (builtins.elemAt darwinArgs 0)
                    && pkgs.lib.hasSuffix "/bin/claude-monitor" (builtins.elemAt darwinArgs 0)
                    && builtins.elemAt darwinArgs 1 == "boot-check"
                  )
                ) "boot-check launchd agent does not run the store-path `claude-monitor boot-check`"
                ++ pkgs.lib.optional (
                  bootCheckDarwin.config.RunAtLoad != true
                ) "boot-check launchd agent is not RunAtLoad"
                ++ pkgs.lib.optional (
                  pkgs.stdenv.hostPlatform.isLinux
                  && builtins.hasAttr bootCheckServiceName
                    (mkHmConfigWith {
                      copilot = false;
                      tmuxHooks = false;
                      bootCheck = true;
                    }).config.launchd.agents
                ) "boot-check launchd agent generated on Linux";
              # banner hook: absent by default; when enabled the tmux config
              # carries one client-attached line that backgrounds the
              # store-path `boot-check --banner` with no shell expansions.
              bannerLines =
                conf:
                builtins.filter (l: pkgs.lib.hasPrefix "set-hook -g client-attached " l) (pkgs.lib.splitString "\n" conf);
              bannerOn = bannerLines (tmuxConf (mkHmConfigWith {
                copilot = false;
                tmuxHooks = false;
                bootCheckBanner = true;
              }));
              bannerProblems =
                pkgs.lib.optional (
                  bannerLines tmuxOff != [ ]
                ) "client-attached hook present with bootCheck.banner.enable = false"
                ++ pkgs.lib.optional (
                  !(
                    builtins.length bannerOn == 1
                    && pkgs.lib.hasInfix "run-shell -b" (builtins.head bannerOn)
                    && pkgs.lib.hasInfix "/nix/store/" (builtins.head bannerOn)
                    && pkgs.lib.hasInfix "/bin/claude-monitor boot-check --banner\"'" (builtins.head bannerOn)
                    && !pkgs.lib.hasInfix "$" (builtins.head bannerOn)
                  )
                ) "client-attached hook missing or not a backgrounded store-path `boot-check --banner`";
              expectedEvents = [
                "SessionStart"
                "UserPromptSubmit"
                "PreToolUse"
                "PostToolUse"
                "PermissionRequest"
                "Notification"
                "Stop"
                "SessionEnd"
              ];
              missing = builtins.filter (event: !(builtins.hasAttr event hooks)) expectedEvents;
            in
            if missing != [ ] then
              throw "claude-monitor hm-module: missing hook events: ${toString missing}"
            else if copilotProblems != [ ] then
              throw "claude-monitor hm-module: ${builtins.concatStringsSep "; " copilotProblems}"
            else if tmuxProblems != [ ] then
              throw "claude-monitor hm-module: ${builtins.concatStringsSep "; " tmuxProblems}"
            else if watchProblems != [ ] then
              throw "claude-monitor hm-module: ${builtins.concatStringsSep "; " watchProblems}"
            else if bootCheckProblems != [ ] then
              throw "claude-monitor hm-module: ${builtins.concatStringsSep "; " bootCheckProblems}"
            else if launchdProblems != [ ] then
              throw "claude-monitor hm-module: ${builtins.concatStringsSep "; " launchdProblems}"
            else if bannerProblems != [ ] then
              throw "claude-monitor hm-module: ${builtins.concatStringsSep "; " bannerProblems}"
            else
              hmPkgs.runCommand "claude-monitor-hm-module-check" { } "touch $out";
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);

      homeManagerModules.default = import ./nix/hm-module.nix { inherit self; };
      homeManagerModules.claude-monitor = self.homeManagerModules.default;
    };
}
