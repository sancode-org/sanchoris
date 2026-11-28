{
  description = "sanchoris: predictable skills inside AI coding agent threads";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    leanfran = {
      url = "github:conao3/lean-leanfran/420a7dce66dda3cbfd03b884d453cb8de63a0fc6";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];

      imports = [ inputs.treefmt-nix.flakeModule ];

      perSystem =
        {
          config,
          lib,
          system,
          ...
        }:
        let
          pkgs = import inputs.nixpkgs {
            inherit system;
            config.allowUnfreePredicate = pkg: lib.getName pkg == "dynamodb-local";
          };
          leanfran = inputs.leanfran.packages.${system}.leanfran;
          nodejs = pkgs.nodejs_26;
          pnpm = pkgs.pnpm.override { nodejs-slim = pkgs.nodejs-slim_26; };
          vp = pkgs.writeShellApplication {
            name = "vp";
            runtimeInputs = [
              nodejs
              pnpm
            ];
            text = ''
              workspace_root="$PWD"
              while [[ "$workspace_root" != / && ! -f "$workspace_root/pnpm-workspace.yaml" ]]; do
                workspace_root="$(dirname "$workspace_root")"
              done

              if [[ ! -f "$workspace_root/pnpm-workspace.yaml" ]]; then
                printf '%s\n' "vp must run inside the sanchoris workspace" >&2
                exit 1
              fi

              if [[ "''${1:-}" == install ]]; then
                shift
                exec pnpm install "$@"
              fi

              vp_cli="$workspace_root/node_modules/vite-plus/bin/vp"
              if [[ ! -f "$vp_cli" ]]; then
                printf '%s\n' "The local Vite+ CLI is missing; run 'vp install' first" >&2
                exit 1
              fi

              export PATH="${nodejs}/bin:$workspace_root/node_modules/.bin:$PATH"
            ''
            + lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
              tsrs_static="$workspace_root/node_modules/.pnpm/node_modules/@tsrs/linux-${
                if pkgs.stdenv.hostPlatform.isAarch64 then "arm64" else "x64"
              }-musl/tsrs"
              if [[ -z "''${TSRS_BINARY:-}" && -x "$tsrs_static" ]]; then
                export TSRS_BINARY="$tsrs_static"
              fi
            ''
            + ''
              exec ${nodejs}/bin/node "$vp_cli" "$@"
            '';
          };
          version = "0.1.0";
          rustSource = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
            ];
          };
          rustPackage =
            pname: package:
            pkgs.rustPlatform.buildRustPackage {
              inherit pname version;
              src = rustSource;
              cargoLock.lockFile = ./Cargo.lock;
              cargoBuildFlags = [
                "-p"
                package
              ];
              doCheck = false;
            };
          server = rustPackage "sanchoris-server" "sanchoris-api";
          driver = (rustPackage "sanchoris-driver" "sanchoris-driver").overrideAttrs {
            meta = {
              description = "Runs sanchoris workspaces on this machine";
              mainProgram = "sanchoris-driver";
            };
          };
          web = pkgs.stdenv.mkDerivation (finalAttrs: {
            pname = "sanchoris-web";
            inherit version;
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./package.json
                ./pnpm-lock.yaml
                ./pnpm-workspace.yaml
                ./vite.config.ts
                ./tsconfig.base.json
                ./tsconfig.json
                ./scripts
                ./apps
              ];
            };
            pnpmDeps = pkgs.fetchPnpmDeps {
              inherit (finalAttrs) pname version src;
              inherit pnpm;
              fetcherVersion = 5;
              hash = "sha256-auTw1AaYwSu/cazCBF/IalouYQ9MkUxu0sUWD3dSeVM=";
            };
            nativeBuildInputs = [
              nodejs
              pnpm
              pkgs.pnpmConfigHook
              pkgs.writableTmpDirAsHomeHook
            ];
            buildPhase = ''
              runHook preBuild
              export PATH="${nodejs}/bin:$PWD/node_modules/.bin:$PATH"
              node node_modules/vite-plus/bin/vp build
              runHook postBuild
            '';
            installPhase = ''
              runHook preInstall
              mkdir -p $out/share/sanchoris
              cp -r apps/frontend/dist $out/share/sanchoris/frontend
              runHook postInstall
            '';
          });
          app = pkgs.writeShellApplication {
            name = "sanchoris";
            runtimeInputs = [
              nodejs
              pkgs.dynamodb-local
            ];
            text = ''
              state="''${XDG_STATE_HOME:-$HOME/.local/state}/sanchoris"
              mkdir -p "$state"

              dynamo_port=$(node -e "const s = require('net').createServer().listen(0, '127.0.0.1', () => { console.log(s.address().port); s.close(); });")
              dynamodb-local -inMemory -disableTelemetry -port "$dynamo_port" >"$state/dynamodb.log" 2>&1 &
              dynamo_pid=$!
              trap 'kill "$dynamo_pid" 2>/dev/null || true' EXIT
              trap 'exit 143' TERM
              trap 'exit 130' INT
              deadline=$((SECONDS + 60))
              until (exec 3<>"/dev/tcp/127.0.0.1/$dynamo_port") 2>/dev/null; do
                if [ "$SECONDS" -ge "$deadline" ]; then
                  echo "sanchoris: DynamoDB Local did not listen on port $dynamo_port (log: $state/dynamodb.log)" >&2
                  exit 1
                fi
                sleep 0.2
              done

              export AWS_REGION=ap-northeast-1
              export AWS_ACCESS_KEY_ID=local
              export AWS_SECRET_ACCESS_KEY=local
              export AWS_ENDPOINT_URL_DYNAMODB="http://127.0.0.1:$dynamo_port"
              export SANCHORIS_BIND_ADDR="''${SANCHORIS_BIND_ADDR:-127.0.0.1:8080}"
              export SANCHORIS_FRONTEND_DIR=${web}/share/sanchoris/frontend

              ${server}/bin/sanchoris &
              server_pid=$!
              trap 'kill "$server_pid" "$dynamo_pid" 2>/dev/null || true' EXIT
              wait "$server_pid"
            '';
            meta = {
              description = "The sanchoris control plane and screens in one local process against an in-memory DynamoDB Local";
              mainProgram = "sanchoris";
            };
          };
        in
        {
          packages = {
            default = app;
            inherit
              server
              driver
              web
              ;
          };

          devShells.default = pkgs.mkShell {
            inputsFrom = [ config.treefmt.build.devShell ];
            packages = [
              pkgs.cargo
              pkgs.rustc
              pkgs.clippy
              pkgs.rustfmt
              pkgs.rust-analyzer
              pkgs.lean4
              leanfran
              nodejs
              pnpm
              vp
              pkgs.dynamodb-local
              pkgs.gnumake
            ];
          };

          treefmt = {
            projectRootFile = "flake.nix";
            programs.nixfmt.enable = true;
            programs.rustfmt = {
              enable = true;
              edition = "2024";
            };
            settings.formatter.rustfmt.excludes = [ "crates/*/src/generated/*" ];
          };
        };
    };
}
