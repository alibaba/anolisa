fn validate_absolute(path: &Path) -> Result<(), AcpV1CodecError> {
    if !path.is_absolute() {
        return Err(AcpV1CodecError::WorkspaceNotAbsolute(path.to_path_buf()));
    }
    Ok(())
}

fn batch_entry_requires_response(entry: &TransportBatchEntry) -> bool {
    match entry {
        TransportBatchEntry::Message(RawJsonRpcMessage::Request(_)) => true,
        TransportBatchEntry::Message(
            RawJsonRpcMessage::Notification(_) | RawJsonRpcMessage::Response(_),
        ) => false,
        TransportBatchEntry::Malformed { raw, .. } => !is_response_only_shape(raw),
    }
}

fn is_response_only_shape(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|object| {
        !object.contains_key("method")
            && (object.contains_key("result") || object.contains_key("error"))
    })
}

fn decode_params<T: serde::de::DeserializeOwned>(
    params: Option<RawJsonRpcParams>,
) -> Result<T, AcpV1CodecError> {
    let value = params.map_or(serde_json::Value::Null, RawJsonRpcParams::into_value);
    serde_json::from_value(value).map_err(Into::into)
}

fn from_sdk_request_id(id: RequestId) -> Result<AcpV1RequestId, AcpV1CodecError> {
    match id {
        RequestId::Null => Err(AcpV1CodecError::NullRequestId),
        RequestId::Number(value) => Ok(AcpV1RequestId::Number(value)),
        RequestId::Str(value) => Ok(AcpV1RequestId::String(value)),
    }
}

fn to_sdk_request_id(id: &AcpV1RequestId) -> RequestId {
    match id {
        AcpV1RequestId::Number(value) => RequestId::Number(*value),
        AcpV1RequestId::String(value) => RequestId::Str(value.clone()),
    }
}

fn copy_capabilities(capabilities: &AgentCapabilities) -> AcpV1AgentCapabilities {
    AcpV1AgentCapabilities {
        load_session: capabilities.load_session,
        list_sessions: capabilities.session_capabilities.list.is_some(),
        delete_session: capabilities.session_capabilities.delete.is_some(),
        additional_directories: capabilities
            .session_capabilities
            .additional_directories
            .is_some(),
        resume_session: capabilities.session_capabilities.resume.is_some(),
        close_session: capabilities.session_capabilities.close.is_some(),
        image_prompts: capabilities.prompt_capabilities.image,
        audio_prompts: capabilities.prompt_capabilities.audio,
        embedded_context: capabilities.prompt_capabilities.embedded_context,
    }
}

fn copy_stop_reason(reason: StopReason) -> AcpV1StopReason {
    match reason {
        StopReason::EndTurn => AcpV1StopReason::EndTurn,
        StopReason::MaxTokens => AcpV1StopReason::MaxTokens,
        StopReason::MaxTurnRequests => AcpV1StopReason::MaxTurnRequests,
        StopReason::Refusal => AcpV1StopReason::Refusal,
        StopReason::Cancelled => AcpV1StopReason::Cancelled,
        _ => AcpV1StopReason::Unsupported,
    }
}

fn copy_permission_kind(kind: PermissionOptionKind) -> AcpV1PermissionOptionKind {
    match kind {
        PermissionOptionKind::AllowOnce => AcpV1PermissionOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways => AcpV1PermissionOptionKind::AllowAlways,
        PermissionOptionKind::RejectOnce => AcpV1PermissionOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways => AcpV1PermissionOptionKind::RejectAlways,
        _ => AcpV1PermissionOptionKind::Unsupported,
    }
}

const _: () = assert!(ProtocolVersion::V1.as_u16() == ACP_WIRE_PROTOCOL_VERSION);
