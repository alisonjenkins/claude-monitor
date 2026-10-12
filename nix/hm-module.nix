{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.claude-monitor;

  # PreToolUse/PostToolUse fire per matched tool name, so the settings schema
  # wants an explicit matcher even for "run on every tool"; the other events
  # have no matcher concept and take a single match-all group.
  toolMatcherEvents = [
    "PreToolUse"
    "PostToolUse"
    "PermissionRequest"
  ];
  hookEvents = [
    "SessionStart"
    "UserPromptSubmit"
    "PreToolUse"
    "PostToolUse"
    "PermissionRequest"
    "Notification"
    "Stop"
    "SessionEnd"
  ];

  hookCommand = "${cfg.package}/bin/claude-monitor hook";

  mkHookGroup =
    event:
    {
      hooks = [
        {
          type = "command";
          command = hookCommand;
        }
      ];
    }
    // lib.optionalAttrs (builtins.elem event toolMatcherEvents) {
      matcher = "*";
    };

  # Copilot CLI hook events are camelCase, unlike Claude's PascalCase ones.
  copilotHookEvents = [
    "sessionStart"
    "sessionEnd"
    "userPromptSubmitted"
    "preToolUse"
    "postToolUse"
    "permissionRequest"
    "notification"
    "agentStop"
  ];

  copilotHooksFile = {
    version = 1;
    hooks = lib.genAttrs copilotHookEvents (event: [
      {
        type = "command";
        bash = "${hookCommand} --agent copilot --event ${event}";
        timeoutSec = 10;
      }
    ]);
  };

  # tmux hooks, verified on tmux 3.6a with a private server (research D12/R9):
  # - pane-exited fires when a pane's process exits and the pane closes
  #   (SIGTERM or SIGKILL), and exposes #{hook_pane}. It does not fire for
  #   kill-pane or kill-window.
  # - pane-died fires instead when remain-on-exit keeps the dead pane, with
  #   #{hook_pane} as well.
  # - window-unlinked fires when a window goes (including kill-pane/kill-window
  #   of its last pane) but exposes only #{hook_window}, never the pane id, so
  #   it runs the full reconcile instead of `--pane`.
  # - None fire when the server itself dies; the login boot-check covers that.
  # Hooks run with the server's environment, so the binary is a store path
  # rather than a PATH lookup. `#{hook_pane}` is expanded by tmux before the
  # shell sees it, so no extra quoting is needed for a `%N` id.
  tmuxHooksSnippet =
    let
      bin = "${cfg.package}/bin/claude-monitor";
    in
    ''
      set-hook -g pane-exited 'run-shell -b "${bin} reconcile --pane #{hook_pane}"'
      set-hook -g pane-died 'run-shell -b "${bin} reconcile --pane #{hook_pane}"'
      set-hook -g window-unlinked 'run-shell -b "${bin} reconcile"'
    '';
in
{
  options.programs.claude-monitor = {
    tmuxHooks = {
      enable = lib.mkEnableOption "tmux hooks that run claude-monitor when a pane exits" // {
        description = ''
          Run `claude-monitor reconcile --pane` when a tmux pane exits, so a
          session in a closed pane is marked interrupted even with no viewer
          open.

          This appends the hooks to `programs.tmux.extraConfig`, so it works
          only when tmux is managed by Home Manager. If you manage `tmux.conf`
          another way, leave this off and include
          `programs.claude-monitor.tmuxHooks.snippet` yourself.
        '';
      };

      snippet = lib.mkOption {
        type = lib.types.str;
        readOnly = true;
        default = tmuxHooksSnippet;
        defaultText = lib.literalMD "the `set-hook` lines, with the store path of `package`";
        description = "The tmux `set-hook` lines, always computed, for tmux configs not managed by Home Manager.";
      };
    };

    bootCheck = {
      enable = lib.mkEnableOption "a login job that runs `claude-monitor boot-check`" // {
        description = ''
          Run `claude-monitor boot-check` once at login (a systemd user unit
          on Linux, a launchd agent on macOS). If a reboot or crash left
          sessions interrupted it sends one desktop notification and leaves a
          marker for the tmux banner. It never starts an agent: restoring
          sessions stays an explicit `claude-monitor restore`.
        '';
      };
    };

    copilot = {
      enable = lib.mkEnableOption "registering claude-monitor as a GitHub Copilot CLI hook handler" // {
        description = ''
          Write a Copilot CLI hooks file so claude-monitor sees Copilot
          sessions as well as Claude Code ones.

          Caveats:

          - Copilot loads hooks only for folders listed in `trustedFolders`
            in `$COPILOT_HOME/config.json` and silently skips them
            otherwise. Sessions in untrusted folders are still discovered
            passively, but their state is unknown.
          - Copilot does not read the user-level `~/.claude/settings.json`
            hooks, so this separate file is required.
          - `sessionStart` fires lazily, on the first prompt, not when the
            session opens.
        '';
      };

      hooksDir = lib.mkOption {
        type = lib.types.str;
        default = ".copilot/hooks";
        example = ".config/copilot/hooks";
        description = ''
          Directory, relative to `$HOME`, that Copilot reads user-level hook
          files from (`$COPILOT_HOME/hooks`). Change it only when
          `COPILOT_HOME` points somewhere other than `~/.copilot`.
        '';
      };
    };

    watch = {
      enable = lib.mkEnableOption "a systemd user service that runs `claude-monitor watch`" // {
        description = ''
          Run `claude-monitor watch` as a systemd user service (Linux only),
          so session states are tracked and logged to the journal, one line
          per state change, even when no dashboard is open. It only reads the
          agents' own directories and never starts an agent.
        '';
      };
    };

    enable = lib.mkEnableOption "claude-monitor, a TUI dashboard for Claude Code sessions in tmux";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "self.packages.<system>.default";
      description = "The claude-monitor package to install and to register the hook command for.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    home.file = lib.mkIf cfg.copilot.enable {
      "${cfg.copilot.hooksDir}/claude-monitor.json".text = builtins.toJSON copilotHooksFile;
    };

    programs.tmux.extraConfig = lib.mkIf cfg.tmuxHooks.enable cfg.tmuxHooks.snippet;

    systemd.user.services.claude-monitor-watch =
      lib.mkIf (cfg.watch.enable && pkgs.stdenv.hostPlatform.isLinux)
        {
          Unit.Description = "claude-monitor: log AI agent session state changes";
          Service = {
            ExecStart = "${cfg.package}/bin/claude-monitor watch";
            Restart = "on-failure";
            RestartSec = 5;
          };
          Install.WantedBy = [ "default.target" ];
        };

    systemd.user.services.claude-monitor-boot-check =
      lib.mkIf (cfg.bootCheck.enable && pkgs.stdenv.hostPlatform.isLinux)
        {
          Unit.Description = "claude-monitor: report AI sessions interrupted by a reboot";
          Service = {
            Type = "oneshot";
            ExecStart = "${cfg.package}/bin/claude-monitor boot-check";
          };
          Install.WantedBy = [ "default.target" ];
        };

    launchd.agents.claude-monitor-boot-check =
      lib.mkIf (cfg.bootCheck.enable && pkgs.stdenv.hostPlatform.isDarwin)
        {
          enable = true;
          config = {
            ProgramArguments = [
              "${cfg.package}/bin/claude-monitor"
              "boot-check"
            ];
            RunAtLoad = true;
          };
        };

    # Merged into the upstream home-manager `programs.claude-code.settings`
    # option (JSON-value type: attrsOf recurses, listOf concatenates), so this
    # adds one hook group per event alongside whatever the user's own config
    # already registers for the same event — it does not replace it.
    programs.claude-code.settings.hooks = lib.genAttrs hookEvents (event: [ (mkHookGroup event) ]);
  };
}
