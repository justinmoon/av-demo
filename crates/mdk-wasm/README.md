# mdk-wasm

Browser-targeted WASM wrapper for MDK/OpenMLS with identity management and structured types.

## Build

### Without MDK (stub only)
```bash
rustup target add wasm32-unknown-unknown
cargo build -p mdk-wasm --target wasm32-unknown-unknown
```

### With MDK (real MLS implementation)
Requires nix shell with WASM C toolchain:
```bash
nix develop
cargo build -p mdk-wasm --target wasm32-unknown-unknown --features with-mdk
```

## API

### Identity Management
- `create_identity(secret_hex: string) -> u32` - Create identity from hex secret, returns identity ID
- `init(user_secret_hex: string) -> u32` - Alias for create_identity (backwards compatibility)

### Key Packages
- `create_key_package(identity_id: u32, relays: JsValue) -> JsValue` - Generate key package
  - Returns: `{ event: string }`

### Group Operations
- `create_group(identity_id: u32, config: JsValue, member_events: JsValue) -> JsValue` - Create MLS group
  - config: `{ name: string, description: string, relays: string[], admins: string[], image_hash?: string, image_key?: string, image_nonce?: string }`
  - member_events: Array of key package events (JSON strings)
  - Returns: `{ group_id_hex: string, nostr_group_id: string, welcome: string[] }`
- `accept_welcome(identity_id: u32, welcome_json: string) -> JsValue` - Join group via Welcome
  - Returns: `{ group_id_hex: string, nostr_group_id: string }`
- `list_groups(identity_id: u32) -> JsValue` - Enumerate known groups
  - Returns: Array of `{ group_id_hex: string, nostr_group_id: string, member_count: number }`

### Messaging
- `create_message(identity_id: u32, input: JsValue) -> Uint8Array` - Encrypt message
  - input: `{ group_id_hex: string, rumor: object }`
  - Returns: Nostr event as UTF-8 JSON bytes
- `ingest_wrapper(identity_id: u32, wrapper: Uint8Array) -> JsValue` - Decrypt/process message
  - Returns: `{ kind: "application"|"proposal"|"commit", message?: object, proposal?: object, commit?: object }`

### Epoch Management
- `self_update(identity_id: u32, group_id_hex: string) -> JsValue` - Create epoch rotation
  - Returns: `{ evolution_event: string, welcome: string[] }`
- `merge_pending_commit(identity_id: u32, group_id_hex: string) -> Result<(), JsValue>` - Merge pending commit after epoch rotation

## Testing

### WASM unit tests
```bash
nix develop .# -c wasm-pack test --node --features with-mdk
```

### Playwright integration tests
```bash
npm install          # once
npm run build:wasm   # regenerates tests/pkg from the latest Rust code
npm test             # Runs Playwright tests
```

## Notes

- Identity IDs are unique handles that persist for the lifetime of the WASM module
- All structured inputs/outputs use serde-wasm-bindgen for type-safe JS/Rust interop
- The `with-mdk` feature requires secp256k1 WASM compilation via nix shell
- Without `with-mdk`, all functions return feature-disabled errors
