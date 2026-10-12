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
    hooks = lib.genAttrs copilotHookEvents (_event: [
      {
        type = "command";
        bash = "${hookCommand} --agent copilot";
        timeoutSec = 10;
      }
    ]);
  };
in
{
  options.programs.claude-monitor = {
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

    # Merged into the upstream home-manager `programs.claude-code.settings`
    # option (JSON-value type: attrsOf recurses, listOf concatenates), so this
    # adds one hook group per event alongside whatever the user's own config
    # already registers for the same event — it does not replace it.
    programs.claude-code.settings.hooks = lib.genAttrs hookEvents (event: [ (mkHookGroup event) ]);
  };
}
