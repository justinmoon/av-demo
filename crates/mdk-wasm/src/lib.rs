use once_cell::sync::OnceCell;
use wasm_bindgen::prelude::*;

// Pull in MDK + storage optionally
#[cfg(feature = "with-mdk")]
use mdk_core::MDK;
#[cfg(feature = "with-mdk")]
use mdk_memory_storage::MdkMemoryStorage;

// Global MDK context stored in WASM module memory
struct Ctx {
    #[cfg(feature = "with-mdk")]
    mdk: MDK<MdkMemoryStorage>,
    user_pubkey_hex: Option<String>,
}

static CTX: OnceCell<Ctx> = OnceCell::new();

#[wasm_bindgen(start)]
pub fn wasm_start() {
    #[cfg(feature = "panic-hook")]
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub fn init(user_pubkey_hex: String) -> Result<(), JsValue> {
    // Initialize global MDK context once
    let _ = CTX.set(Ctx {
        #[cfg(feature = "with-mdk")]
        mdk: MDK::new(MdkMemoryStorage::default()),
        user_pubkey_hex: Some(user_pubkey_hex),
    });

    Ok(())
}

#[wasm_bindgen]
pub fn ingest_wrapper(json_bytes: js_sys::Uint8Array) -> Result<JsValue, JsValue> {
    // Minimal placeholder: we don't process yet in Step 1; ensure dependency compiles
    let _ctx = CTX
        .get()
        .ok_or_else(|| JsValue::from_str("mdk-wasm not initialized; call init() first"))?;

    let len = json_bytes.length() as usize;
    let mut buf = vec![0u8; len];
    json_bytes.copy_to(&mut buf[..]);

    // For now, we just validate it's JSON and return unprocessable kind.
    let _v: serde_json::Value = serde_json::from_slice(&buf)
        .map_err(|e| JsValue::from_str(&format!("invalid wrapper JSON: {e}")))?;

    let resp = serde_json::json!({
        "kind": "unprocessable"
    });
    Ok(JsValue::from_str(&resp.to_string()))
}

#[wasm_bindgen]
pub fn create_message(rumor_json: String) -> Result<js_sys::Uint8Array, JsValue> {
    // Minimal placeholder: only parse JSON to ensure serde present
    let _ctx = CTX
        .get()
        .ok_or_else(|| JsValue::from_str("mdk-wasm not initialized; call init() first"))?;
    let _v: serde_json::Value = serde_json::from_str(&rumor_json)
        .map_err(|e| JsValue::from_str(&format!("invalid rumor json: {e}")))?;

    // Step 1 target is compilation; return empty bytes for now.
    Ok(js_sys::Uint8Array::new_with_length(0))
}
