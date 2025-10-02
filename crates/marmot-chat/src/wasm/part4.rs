#[wasm_bindgen]
pub fn ingest_wrapper(identity_id: u32, wrapper: Uint8Array) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let mut bytes = vec![0u8; wrapper.length() as usize];
        wrapper.copy_to(&mut bytes[..]);
        let json = String::from_utf8(bytes).map_err(|e| js_error(format!("invalid utf8: {e}")))?;
        let event =
            Event::from_json(&json).map_err(|e| js_error(format!("invalid wrapper: {e}")))?;
        let result = identity
            .mdk
            .process_message(&event)
            .map_err(|e| js_error(format!("failed to process wrapper: {e}")))?;

        let processed = match result {
            MessageProcessingResult::ApplicationMessage(msg) => ProcessedWrapper {
                kind: "application".into(),
                message: Some(DecryptedMessage {
                    content: msg.content,
                    author: msg.pubkey.to_hex(),
                    created_at: msg.created_at.as_u64(),
                    event: JsonValue::Null,
                }),
                proposal: None,
                commit: None,
            },
            MessageProcessingResult::Commit => ProcessedWrapper {
                kind: "commit".into(),
                message: None,
                proposal: None,
                commit: Some(CommitEnvelope {
                    event: event.as_json(),
                }),
            },
            MessageProcessingResult::Proposal(_) => ProcessedWrapper {
                kind: "proposal".into(),
                message: None,
                proposal: None,
                commit: None,
            },
            MessageProcessingResult::ExternalJoinProposal => ProcessedWrapper {
                kind: "external_join".into(),
                message: None,
                proposal: None,
                commit: None,
            },
            MessageProcessingResult::Unprocessable => ProcessedWrapper {
                kind: "unprocessable".into(),
                message: None,
                proposal: None,
                commit: None,
            },
        };
        swb::to_value(&processed)
            .map_err(|e| js_error(format!("failed to serialize processed wrapper: {e}")))
    })
}

#[wasm_bindgen]
pub fn self_update(identity_id: u32, group_id_hex: String) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let group_id_bytes = decode_hex(&group_id_hex)?;
        let group_id = GroupId::from_slice(&group_id_bytes);
        let update = identity
            .mdk
            .self_update(&group_id)
            .map_err(|e| js_error(format!("failed to self-update: {e}")))?;

        let welcome = update
            .welcome_rumors
            .map(|rumors| rumors.iter().map(|r| r.as_json()).collect());
        let resp = SelfUpdateResult {
            evolution_event: update.evolution_event.as_json(),
            welcome,
        };
        swb::to_value(&resp).map_err(|e| js_error(format!("failed to serialize self update: {e}")))
    })
}

#[wasm_bindgen]
pub fn merge_pending_commit(identity_id: u32, group_id_hex: String) -> Result<(), JsValue> {
    with_identity(identity_id, |identity| {
        let group_id_bytes = decode_hex(&group_id_hex)?;
        let group_id = GroupId::from_slice(&group_id_bytes);
        identity
            .mdk
            .merge_pending_commit(&group_id)
            .map_err(|e| js_error(format!("failed to merge pending commit: {e}")))?;
        Ok(())
    })
}
