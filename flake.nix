{
  description = "MoQ AV demo development shell (Rust + wasm32 tooling)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system; overlays = overlays; };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ];
        };

        wasmClang = pkgs.writeShellScriptBin "clang-wasm32" ''
          unset SDKROOT
          unset MACOSX_DEPLOYMENT_TARGET
          unset NIX_CFLAGS_COMPILE
          unset NIX_CFLAGS_COMPILE_FOR_TARGET
          unset NIX_CXXFLAGS_COMPILE
          unset NIX_LDFLAGS
          filtered=()
          while [ "$#" -gt 0 ]; do
            case "$1" in
              -arch)
                shift 2
                continue
                ;;
              -mmacos-version-min=*)
                shift
                continue
                ;;
              -mmacosx-version-min=*)
                shift
                continue
                ;;
              -isysroot)
                shift 2
                continue
                ;;
            esac
            filtered+=("$1")
            shift
          done
          set -- ''${filtered[@]}
          exec ${pkgs.llvmPackages_18.clang-unwrapped}/bin/clang --target=wasm32-unknown-unknown "$@"
        '';

        wasmClangxx = pkgs.writeShellScriptBin "clang++-wasm32" ''
          unset SDKROOT
          unset MACOSX_DEPLOYMENT_TARGET
          unset NIX_CFLAGS_COMPILE
          unset NIX_CFLAGS_COMPILE_FOR_TARGET
          unset NIX_CXXFLAGS_COMPILE
          unset NIX_LDFLAGS
          filtered=()
          while [ "$#" -gt 0 ]; do
            case "$1" in
              -arch)
                shift 2
                continue
                ;;
              -mmacos-version-min=*)
                shift
                continue
                ;;
              -mmacosx-version-min=*)
                shift
                continue
                ;;
              -isysroot)
                shift 2
                continue
                ;;
            esac
            filtered+=("$1")
            shift
          done
          set -- ''${filtered[@]}
          exec ${pkgs.llvmPackages_18.clang-unwrapped}/bin/clang++ --target=wasm32-unknown-unknown "$@"
        '';

        wasmAr = pkgs.writeShellScriptBin "llvm-ar-wasm32" ''
          unset SDKROOT
          unset MACOSX_DEPLOYMENT_TARGET
          unset NIX_CFLAGS_COMPILE
          unset NIX_CFLAGS_COMPILE_FOR_TARGET
          unset NIX_LDFLAGS
          exec ${pkgs.llvmPackages_18.llvm}/bin/llvm-ar "$@"
        '';
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            pkgs.darwin.apple_sdk.frameworks.CoreServices
            pkgs.darwin.apple_sdk.frameworks.Foundation
            pkgs.darwin.apple_sdk.frameworks.Security
          ];

          packages = [
            rustToolchain
            pkgs.rust-analyzer
            pkgs.cargo-nextest
            pkgs.binaryen
            pkgs.wasm-bindgen-cli
            pkgs.wasm-pack
            pkgs.pkg-config
            pkgs.llvmPackages_18.clang
            pkgs.llvmPackages_18.clang-unwrapped
            pkgs.llvmPackages_18.llvm
            pkgs.nostr-rs-relay
            wasmClang
            wasmClangxx
            wasmAr
          ];

          shellHook = ''
            export CC_wasm32_unknown_unknown=clang-wasm32
            export CXX_wasm32_unknown_unknown=clang++-wasm32
            export AR_wasm32_unknown_unknown=llvm-ar-wasm32
            export CFLAGS_wasm32_unknown_unknown="-I ${pkgs.llvmPackages_18.libclang.lib}/lib/clang/18/include"
            export CXXFLAGS_wasm32_unknown_unknown="$CFLAGS_wasm32_unknown_unknown"
            export BINDGEN_EXTRA_CLANG_ARGS_wasm32_unknown_unknown="$CFLAGS_wasm32_unknown_unknown"
          '';
        };
      }
    );
}
