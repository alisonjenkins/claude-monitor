{
  description = "Claude Code session monitor - TUI dashboard for managing multiple Claude sessions in tmux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
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
        version = "0.1.0";
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
          default = pkgs.rustPlatform.buildRustPackage (cargoBaseArgs // rustEnvFor pkgs);
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
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);

      homeManagerModules.default = import ./nix/hm-module.nix { inherit self; };
      homeManagerModules.claude-monitor = self.homeManagerModules.default;
    };
}
