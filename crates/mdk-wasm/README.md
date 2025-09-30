mdk-wasm

Browser-targeted WASM wrapper for MDK/OpenMLS.

Build
- rustup target add wasm32-unknown-unknown
- cargo build -p mdk-wasm --target wasm32-unknown-unknown

Notes
- The MDK dependencies are behind the `with-mdk` feature to avoid building `secp256k1-sys` until a WASM C toolchain is configured.
- Once a clang/emscripten toolchain capable of compiling C to `wasm32-unknown-unknown` is present, enable MDK with:
  - cargo build -p mdk-wasm --target wasm32-unknown-unknown --features with-mdk

Exports (MVP stubs)
- init(user_pubkey_hex: string)
- ingest_wrapper(json_bytes: Uint8Array) -> { kind: "unprocessable" }
- create_message(rumor_json: string) -> Uint8Array

