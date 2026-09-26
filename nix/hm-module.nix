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
in
{
  options.programs.claude-monitor = {
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

    # Merged into the upstream home-manager `programs.claude-code.settings`
    # option (JSON-value type: attrsOf recurses, listOf concatenates), so this
    # adds one hook group per event alongside whatever the user's own config
    # already registers for the same event — it does not replace it.
    programs.claude-code.settings.hooks = lib.genAttrs hookEvents (event: [ (mkHookGroup event) ]);
  };
}
