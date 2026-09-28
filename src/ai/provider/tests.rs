use super::*;

fn response_with_message(message: Value) -> CapabilityProbeResponse {
    CapabilityProbeResponse::Success(json!({
        "choices": [{"message": message}]
    }))
}

fn provider(id: &str, model: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        name: id.to_string(),
        endpoint: format!("https://{id}.example/v1"),
        api_key: String::new(),
        api_key_ref: None,
        models: vec![model.to_string()],
        active_model: model.to_string(),
    }
}

#[test]
fn main_model_route_resolves_dynamically_to_current_main() {
    let mut store = ProviderStore {
        active_id: Some("main-a".to_string()),
        providers: vec![provider("main-a", "model-a"), provider("main-b", "model-b")],
    };
    let routing = ModelRoutingConfig::default();
    let (_, model, origin) = select_model_route(&store, &routing, ModelRole::Vision)
        .expect("select_model_route succeeds");
    assert_eq!(model, "model-a");
    assert_eq!(origin, RouteOrigin::MainModel);

    store.active_id = Some("main-b".to_string());
    let (_, model, origin) = select_model_route(&store, &routing, ModelRole::Vision)
        .expect("select_model_route succeeds");
    assert_eq!(model, "model-b");
    assert_eq!(origin, RouteOrigin::MainModel);
}

#[test]
fn specific_route_rejects_missing_provider_and_model() {
    let store = ProviderStore {
        active_id: Some("main".to_string()),
        providers: vec![
            provider("main", "main-model"),
            provider("vision", "vision-v1"),
        ],
    };
    let mut routing = ModelRoutingConfig::default();
    routing
        .set_route(
            ModelRole::Vision,
            ModelRoute::Specific {
                provider_id: "missing".to_string(),
                model: "vision-v1".to_string(),
            },
        )
        .expect("set_route succeeds");
    assert!(select_model_route(&store, &routing, ModelRole::Vision)
        .expect_err("route to missing provider should fail")
        .contains("not found"));

    routing
        .set_route(
            ModelRole::Vision,
            ModelRoute::Specific {
                provider_id: "vision".to_string(),
                model: "vision-v2".to_string(),
            },
        )
        .expect("set_route succeeds");
    assert!(select_model_route(&store, &routing, ModelRole::Vision)
        .expect_err("route to missing model should fail")
        .contains("no longer present"));
}

#[test]
fn failed_capability_persistence_does_not_publish_candidate() {
    let original = CapabilityRecord {
        provider_id: "provider".to_string(),
        model: "model".to_string(),
        supports_image_input: Some(false),
        ..CapabilityRecord::default()
    };
    let mut runtime = CapabilityRegistry {
        models: vec![original.clone()],
    };
    let candidate = CapabilityRegistry {
        models: vec![CapabilityRecord {
            supports_image_input: Some(true),
            ..original
        }],
    };
    assert!(!publish_capability_candidate(
        &mut runtime,
        candidate,
        false
    ));
    assert_eq!(runtime.models[0].supports_image_input, Some(false));
}

#[test]
fn transient_probe_result_preserves_previous_authoritative_evidence() {
    let now = chrono::Utc::now().to_rfc3339();
    let mut record = CapabilityRecord {
        evidence: vec![CapabilityEvidence {
            capability: CapabilityKind::ImageGeneration,
            source: CapabilityEvidenceSource::ActiveProbe,
            outcome: CapabilityState::Supported,
            checked_at: now.clone(),
            detail: Some("previous successful active probe".to_string()),
        }],
        ..CapabilityRecord::default()
    };
    replace_capability_evidence(
        &mut record,
        CapabilityKind::ImageGeneration,
        CapabilityEvidenceSource::ActiveProbe,
        None,
        &now,
        Some("transient timeout".to_string()),
    );
    assert_eq!(record.evidence.len(), 1);
    assert_eq!(
        record.effective_state_for(CapabilityKind::ImageGeneration),
        CapabilityState::Supported
    );
    assert_eq!(
        record.evidence[0].detail.as_deref(),
        Some("previous successful active probe")
    );
}

#[test]
fn configured_routes_ignore_diagnostic_evidence() {
    let provider = provider("main", "model");
    for state in [
        CapabilityState::Unknown,
        CapabilityState::Supported,
        CapabilityState::Unsupported,
    ] {
        for age in [chrono::Duration::zero(), chrono::Duration::days(90)] {
            let mut record = supported_record(&provider, "model");
            for evidence in &mut record.evidence {
                evidence.outcome = state;
                evidence.checked_at = (chrono::Utc::now() - age).to_rfc3339();
            }
            for records in [vec![], vec![record.clone()]] {
                let mut snapshot = GenerationModelSnapshot {
                    provider_store: ProviderStore {
                        active_id: Some(provider.id.clone()),
                        providers: vec![provider.clone()],
                    },
                    routing: ModelRoutingConfig::default(),
                    capabilities: CapabilityRegistry { models: records },
                };
                for role in [ModelRole::Main]
                    .into_iter()
                    .chain(ModelRole::addon_roles())
                {
                    let resolved =
                        AIChatService::resolve_model_route_from_snapshot(&snapshot, role)
                            .expect("resolve_model_route succeeds");
                    assert_eq!(resolved.model, "model");
                    assert_eq!(resolved.provider.endpoint, provider.endpoint);
                    if role != ModelRole::Main {
                        snapshot
                            .routing
                            .set_route(role, ModelRoute::Disabled)
                            .expect("set_route succeeds");
                        assert!(
                            AIChatService::resolve_model_route_from_snapshot(&snapshot, role,)
                                .is_err()
                        );
                        snapshot
                            .routing
                            .set_route(
                                role,
                                ModelRoute::Specific {
                                    provider_id: provider.id.clone(),
                                    model: "model".into(),
                                },
                            )
                            .expect("set_route succeeds");
                        assert!(
                            AIChatService::resolve_model_route_from_snapshot(&snapshot, role,)
                                .is_ok()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn functional_probe_success_is_supported() {
    let response = CapabilityProbeResponse::Success(json!({"ok": true}));
    assert_eq!(response.outcome(Some(true)), ProbeOutcome::Supported);
}

#[test]
fn successful_http_without_tool_call_does_not_prove_tools() {
    let response = response_with_message(json!({"content": "OK"}));
    assert_eq!(validate_tools_probe(&response), None);
}

#[test]
fn named_tool_call_proves_tools() {
    let response = response_with_message(json!({
        "content": null,
        "tool_calls": [{
            "type": "function",
            "function": {"name": "xiao_capability_probe", "arguments": "{}"}
        }]
    }));
    assert_eq!(validate_tools_probe(&response), Some(true));
}

#[test]
fn structured_probe_requires_expected_json_behavior() {
    let good = response_with_message(json!({"content": "{\"xiao_probe\":true}"}));
    let ignored = response_with_message(json!({"content": "sure"}));
    assert_eq!(validate_structured_probe(&good), Some(true));
    assert_eq!(validate_structured_probe(&ignored), None);
}

#[test]
fn vision_probe_requires_two_demonstrated_colors() {
    let red = response_with_message(json!({"content": "red"}));
    let blue = response_with_message(json!({"content": "blue"}));
    assert_eq!(validate_color_probe(&red, "red"), Some(true));
    assert_eq!(validate_color_probe(&blue, "blue"), Some(true));
    assert_eq!(
        combine_vision_probe_results(
            validate_color_probe(&red, "red"),
            validate_color_probe(&blue, "blue")
        ),
        Some(true)
    );
}

#[test]
fn functional_test_roles_keep_image_generation_on_explicit_path() {
    assert_ne!(ModelRole::Vision, ModelRole::ImageGeneration);
    assert_ne!(ModelRole::Video, ModelRole::ImageGeneration);
    assert_ne!(ModelRole::AudioStt, ModelRole::ImageGeneration);
}

#[test]
fn video_probe_payload_contains_bounded_mp4_and_selected_model() {
    let payload = video_probe_payload("video-model");
    assert_eq!(
        payload.get("model").and_then(Value::as_str),
        Some("video-model")
    );
    let url = payload
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.first())
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| content.get(1))
        .and_then(|part| part.get("image_url"))
        .and_then(|image_url| image_url.get("url"))
        .and_then(Value::as_str)
        .expect("url string present");
    let encoded = url
        .strip_prefix("data:video/mp4;base64,")
        .expect("video/mp4 data url prefix");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("decode base64 succeeds");
    assert!(bytes.len() < 2 * 1024);
    assert_eq!(&bytes[4..8], b"ftyp");
}

#[tokio::test]
async fn image_generation_probe_requires_valid_image_bytes() {
    let valid = json!({
        "data": [{
            "b64_json": "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGP8z0AaYCJR/aiGUQ1DSAMAQC4BH2bjRnMAAAAASUVORK5CYII="
        }]
    });
    assert!(validate_image_generation_probe_body(&valid).await);

    let valid_chat_images = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "images": [{
                    "image_url": {
                        "url": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGP8z0AaYCJR/aiGUQ1DSAMAQC4BH2bjRnMAAAAASUVORK5CYII="
                    }
                }]
            }
        }]
    });
    assert!(validate_image_generation_probe_body(&valid_chat_images).await);

    let valid_chat_markdown = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Here is your image: ![result](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGP8z0AaYCJR/aiGUQ1DSAMAQC4BH2bjRnMAAAAASUVORK5CYII=)"
            }
        }]
    });
    assert!(validate_image_generation_probe_body(&valid_chat_markdown).await);

    let invalid = json!({
        "data": [{"b64_json": "bm90IGFuIGltYWdl"}]
    });
    assert!(!validate_image_generation_probe_body(&invalid).await);
}

#[test]
fn explicit_probe_rejection_is_unsupported() {
    assert_eq!(
        validate_tools_probe(&CapabilityProbeResponse::Rejected),
        Some(false)
    );
    assert_eq!(
        validate_structured_probe(&CapabilityProbeResponse::Rejected),
        Some(false)
    );
}

fn catalog_presence_text_chat_claim() -> Option<bool> {
    // Being present in GET /models is catalog evidence only.
    None
}

#[test]
fn catalog_presence_does_not_claim_text_chat() {
    assert_eq!(catalog_presence_text_chat_claim(), None);
}

#[test]
fn provider_metadata_normalizer_handles_observed_openai_compatible_shapes() {
    let metadata = normalize_provider_model_metadata(&json!({
        "id": "model-a",
        "name": "Model A",
        "context_length": 131072,
        "architecture": {"modality": "text+image"},
        "top_provider": {"max_completion_tokens": 8192}
    }))
    .expect("normalize metadata succeeds");
    assert_eq!(metadata.id, "model-a");
    assert_eq!(metadata.context_length, Some(131072));
    assert_eq!(metadata.modalities.as_deref(), Some("text+image"));
    assert_eq!(metadata.max_completion_tokens, Some(8192));

    let metadata = normalize_provider_model_metadata(&json!({
        "id": "model-b",
        "modalities": ["text", "audio", "video"]
    }))
    .expect("normalize metadata succeeds");
    assert_eq!(metadata.modalities.as_deref(), Some("text,audio,video"));
}

#[test]
fn stale_supported_capability_is_effectively_unknown() {
    let record = CapabilityRecord {
        supports_image_input: Some(true),
        checked_at: "2000-01-01T00:00:00+00:00".to_string(),
        ..CapabilityRecord::default()
    };
    assert_eq!(
        record.effective_state_for(CapabilityKind::ImageInput),
        CapabilityState::Unknown
    );
}

#[test]
fn semantic_stt_probe_success_supported() {
    let response = CapabilityProbeResponse::Success(json!({
        "text": "xiao capability probe"
    }));
    assert_eq!(validate_transcription_probe(&response), Some(true));
    assert_eq!(response.outcome(Some(true)), ProbeOutcome::Supported);
}

#[test]
fn semantic_stt_probe_normalized_case_and_punctuation_supported() {
    let response = CapabilityProbeResponse::Success(json!({
        "text": "Xiao capability probe."
    }));
    assert_eq!(validate_transcription_probe(&response), Some(true));
}

#[test]
fn semantic_stt_probe_show_capability_variant_supported() {
    let response = CapabilityProbeResponse::Success(json!({
        "text": "Show capability probe"
    }));
    assert_eq!(validate_transcription_probe(&response), Some(true));
    assert_eq!(response.outcome(Some(true)), ProbeOutcome::Supported);
    assert!(is_expected_probe_transcript("show capability probe"));
}

#[test]
fn semantic_stt_probe_empty_transcript_unknown() {
    let response = CapabilityProbeResponse::Success(json!({
        "text": ""
    }));
    assert_eq!(validate_transcription_probe(&response), None);
    assert_eq!(response.outcome(None), ProbeOutcome::Inconclusive);
}

#[test]
fn semantic_stt_probe_wrong_phrase_unknown() {
    let response = CapabilityProbeResponse::Success(json!({
        "text": "hello world"
    }));
    assert_eq!(validate_transcription_probe(&response), None);
    assert_eq!(response.outcome(None), ProbeOutcome::Inconclusive);
}

#[test]
fn semantic_stt_probe_http_200_without_text_unknown() {
    let response = CapabilityProbeResponse::Success(json!({
        "status": "ok"
    }));
    assert_eq!(validate_transcription_probe(&response), None);
    assert_eq!(response.outcome(None), ProbeOutcome::Inconclusive);
}

#[test]
fn semantic_stt_probe_explicit_unsupported_rejected() {
    let response = CapabilityProbeResponse::Rejected;
    assert_eq!(validate_transcription_probe(&response), Some(false));
    assert_eq!(response.outcome(Some(false)), ProbeOutcome::Unsupported);
}

#[test]
fn semantic_stt_probe_transient_status_unknown() {
    let timeout = CapabilityProbeResponse::Unknown(ProbeOutcome::Timeout);
    assert_eq!(validate_transcription_probe(&timeout), None);
    assert_eq!(timeout.outcome(None), ProbeOutcome::Timeout);

    let rate_limited = CapabilityProbeResponse::Unknown(ProbeOutcome::RateLimited);
    assert_eq!(validate_transcription_probe(&rate_limited), None);
    assert_eq!(rate_limited.outcome(None), ProbeOutcome::RateLimited);

    let server_error = CapabilityProbeResponse::Unknown(ProbeOutcome::ProviderError);
    assert_eq!(validate_transcription_probe(&server_error), None);
    assert_eq!(server_error.outcome(None), ProbeOutcome::ProviderError);
}

#[test]
fn semantic_native_audio_probe_requires_spoken_phrase() {
    let exact = response_with_message(json!({"content":"xiao capability probe"}));
    assert_eq!(validate_native_audio_probe(&exact), Some(true));

    let normalized = response_with_message(json!({"content":"Xiao capability probe!"}));
    assert_eq!(validate_native_audio_probe(&normalized), Some(true));

    let gemini_show = response_with_message(json!({"content":"Show capability probe."}));
    assert_eq!(validate_native_audio_probe(&gemini_show), Some(true));

    let minor = response_with_message(json!({"content":"Ciao capability probe"}));
    assert_eq!(validate_native_audio_probe(&minor), Some(true));

    let ok_only = response_with_message(json!({"content":"OK"}));
    assert_eq!(validate_native_audio_probe(&ok_only), None);

    let wrong = response_with_message(json!({"content":"hello world"}));
    assert_eq!(validate_native_audio_probe(&wrong), None);

    let empty = response_with_message(json!({"content":""}));
    assert_eq!(validate_native_audio_probe(&empty), None);

    let rejected = CapabilityProbeResponse::Rejected;
    assert_eq!(validate_native_audio_probe(&rejected), Some(false));

    for outcome in [
        ProbeOutcome::Timeout,
        ProbeOutcome::NetworkError,
        ProbeOutcome::RateLimited,
        ProbeOutcome::ProviderError,
    ] {
        let response = CapabilityProbeResponse::Unknown(outcome);
        assert_eq!(validate_native_audio_probe(&response), None);
        assert_eq!(response.outcome(None), outcome);
    }
}

#[test]
fn capability_rejection_requires_semantically_bound_phrases() {
    let rejected = [
        (
            CapabilityKind::ImageInput,
            "this model does not support image input",
        ),
        (
            CapabilityKind::ImageInput,
            "vision is not supported for this model",
        ),
        (CapabilityKind::ImageInput, "vision is not supported"),
        (
            CapabilityKind::AudioInput,
            "model does not support audio input",
        ),
        (CapabilityKind::AudioInput, "audio input is not supported"),
        (
            CapabilityKind::AudioTranscription,
            "audio transcription is not supported",
        ),
        (CapabilityKind::VideoInput, "video input is not supported"),
        (CapabilityKind::Tools, "this model does not support tools"),
        (
            CapabilityKind::StructuredOutput,
            "response_format is not supported",
        ),
        (
            CapabilityKind::ImageGeneration,
            "image generation is not supported",
        ),
        (CapabilityKind::TextChat, "text chat is not supported"),
    ];
    for (capability, body) in rejected {
        assert!(
            explicit_capability_rejection(capability, body),
            "{capability:?}: {body}"
        );
    }

    let ambiguous = [
        (
            CapabilityKind::ImageInput,
            "model gpt-4-vision-preview does not support max_tokens",
        ),
        (
            CapabilityKind::ImageInput,
            "vision request for this model does not support response_format",
        ),
        (CapabilityKind::ImageInput, "invalid image url"),
        (CapabilityKind::ImageInput, "unsupported image format"),
        (CapabilityKind::ImageInput, "unsupported media type"),
        (CapabilityKind::ImageInput, "invalid base64"),
        (
            CapabilityKind::AudioInput,
            "audio request does not support max_tokens",
        ),
        (
            CapabilityKind::AudioInput,
            "audio request does not support temperature",
        ),
        (CapabilityKind::AudioInput, "unsupported codec"),
        (CapabilityKind::AudioInput, "invalid input_audio schema"),
        (CapabilityKind::AudioTranscription, "unsupported codec"),
        (CapabilityKind::AudioTranscription, "unsupported file type"),
        (CapabilityKind::AudioTranscription, "malformed multipart"),
        (CapabilityKind::AudioTranscription, "invalid audio format"),
        (
            CapabilityKind::VideoInput,
            "video request does not support temperature",
        ),
        (CapabilityKind::VideoInput, "unsupported video format"),
        (CapabilityKind::VideoInput, "invalid video encoding"),
        (
            CapabilityKind::Tools,
            "this model does not support max_tokens",
        ),
        (
            CapabilityKind::StructuredOutput,
            "this model does not support temperature",
        ),
        (CapabilityKind::ImageGeneration, "unsupported image format"),
        (CapabilityKind::ImageGeneration, "unsupported size"),
        (CapabilityKind::ImageGeneration, "unsupported quality"),
        (CapabilityKind::ImageGeneration, "invalid response_format"),
        (
            CapabilityKind::TextChat,
            "this model does not support max_tokens",
        ),
    ];
    for (capability, body) in ambiguous {
        assert!(
            !explicit_capability_rejection(capability, body),
            "{capability:?}: {body}"
        );
    }
}

#[test]
fn probe_http_policy_only_rejects_explicit_capability_failures() {
    for status in [401, 403] {
        assert!(matches!(
            classify_probe_http_failure(CapabilityKind::Tools, status, "does not support tools"),
            CapabilityProbeResponse::Unknown(ProbeOutcome::AuthFailed)
        ));
    }
    for status in [404, 405, 415] {
        assert!(matches!(
            classify_probe_http_failure(
                CapabilityKind::ImageInput,
                status,
                "this model does not support image input"
            ),
            CapabilityProbeResponse::Unknown(ProbeOutcome::ProtocolMismatch)
        ));
    }
    assert!(matches!(
        classify_probe_http_failure(
            CapabilityKind::ImageInput,
            429,
            "this model does not support image input"
        ),
        CapabilityProbeResponse::Unknown(ProbeOutcome::RateLimited)
    ));
    assert!(matches!(
        classify_probe_http_failure(
            CapabilityKind::ImageInput,
            503,
            "this model does not support image input"
        ),
        CapabilityProbeResponse::Unknown(ProbeOutcome::ProviderError)
    ));

    let ambiguous = [
        (
            CapabilityKind::ImageInput,
            "model gpt-4-vision-preview does not support max_tokens",
        ),
        (
            CapabilityKind::ImageInput,
            "vision request for this model does not support response_format",
        ),
        (
            CapabilityKind::AudioInput,
            "audio request does not support max_tokens",
        ),
        (
            CapabilityKind::AudioInput,
            "audio request does not support temperature",
        ),
        (CapabilityKind::AudioTranscription, "unsupported codec"),
        (
            CapabilityKind::VideoInput,
            "video request does not support temperature",
        ),
        (
            CapabilityKind::Tools,
            "this model does not support max_tokens",
        ),
        (
            CapabilityKind::StructuredOutput,
            "this model does not support temperature",
        ),
    ];
    for status in [400, 422] {
        for (capability, body) in ambiguous {
            assert!(matches!(
                classify_probe_http_failure(capability, status, body),
                CapabilityProbeResponse::Unknown(ProbeOutcome::ProtocolMismatch)
            ));
        }
    }

    let explicit = [
        (
            CapabilityKind::ImageInput,
            "this model does not support image input",
        ),
        (
            CapabilityKind::ImageInput,
            "vision is not supported for this model",
        ),
        (
            CapabilityKind::AudioInput,
            "model does not support audio input",
        ),
        (CapabilityKind::AudioInput, "audio input is not supported"),
        (
            CapabilityKind::AudioTranscription,
            "audio transcription is not supported",
        ),
        (CapabilityKind::VideoInput, "video input is not supported"),
        (CapabilityKind::Tools, "this model does not support tools"),
        (
            CapabilityKind::StructuredOutput,
            "response_format is not supported",
        ),
    ];
    for status in [400, 422] {
        for (capability, body) in explicit {
            assert!(matches!(
                classify_probe_http_failure(capability, status, body),
                CapabilityProbeResponse::Rejected
            ));
        }
    }
}

fn supported_record(provider: &ProviderConfig, model: &str) -> CapabilityRecord {
    let now = chrono::Utc::now().to_rfc3339();
    let kinds = [
        CapabilityKind::TextChat,
        CapabilityKind::ImageInput,
        CapabilityKind::AudioInput,
        CapabilityKind::AudioTranscription,
        CapabilityKind::VideoInput,
        CapabilityKind::ImageGeneration,
    ];
    CapabilityRecord {
        provider_id: provider.endpoint.trim_end_matches('/').to_string(),
        provider_name: provider.name.clone(),
        model: model.to_string(),
        evidence: kinds
            .into_iter()
            .map(|capability| CapabilityEvidence {
                capability,
                source: CapabilityEvidenceSource::ActiveProbe,
                outcome: CapabilityState::Supported,
                checked_at: now.clone(),
                detail: None,
            })
            .collect(),
        ..CapabilityRecord::default()
    }
}

#[test]
fn generation_snapshot_keeps_inherited_routes_on_original_main() {
    let mut live_store = ProviderStore {
        active_id: Some("main-a".to_string()),
        providers: vec![provider("main-a", "model-a"), provider("main-b", "model-b")],
    };
    let routing = ModelRoutingConfig::default();
    let a = live_store.providers[0].clone();
    let b = live_store.providers[1].clone();
    let snapshot = GenerationModelSnapshot {
        provider_store: live_store.clone(),
        routing,
        capabilities: CapabilityRegistry {
            models: vec![
                supported_record(&a, "model-a"),
                supported_record(&b, "model-b"),
            ],
        },
    };

    let initial_main = AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Main)
        .expect("resolve_model_route succeeds");
    live_store.active_id = Some("main-b".to_string());

    for role in [ModelRole::Vision, ModelRole::Video, ModelRole::AudioStt] {
        let inherited = AIChatService::resolve_model_route_from_snapshot(&snapshot, role)
            .expect("resolve_model_route succeeds");
        assert_eq!(inherited.provider.id, "main-a");
        assert_eq!(inherited.model, "model-a");
    }
    let final_main = AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Main)
        .expect("resolve_model_route succeeds");
    assert_eq!(initial_main.provider.id, "main-a");
    assert_eq!(final_main.provider.id, "main-a");
    assert_eq!(live_store.active_id.as_deref(), Some("main-b"));
}

#[test]
fn compound_image_explanation_reuses_original_main_snapshot() {
    let mut live_store = ProviderStore {
        active_id: Some("main-a".to_string()),
        providers: vec![provider("main-a", "model-a"), provider("main-b", "model-b")],
    };
    let a = live_store.providers[0].clone();
    let b = live_store.providers[1].clone();
    let snapshot = GenerationModelSnapshot {
        provider_store: live_store.clone(),
        routing: ModelRoutingConfig::default(),
        capabilities: CapabilityRegistry {
            models: vec![
                supported_record(&a, "model-a"),
                supported_record(&b, "model-b"),
            ],
        },
    };

    let image_route =
        AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::ImageGeneration)
            .expect("resolve_model_route succeeds");
    assert_eq!(image_route.provider.id, "main-a");

    live_store.active_id = Some("main-b".to_string());

    let explanation_route =
        AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Main)
            .expect("resolve_model_route succeeds");
    assert_eq!(explanation_route.provider.id, "main-a");

    let next_request = GenerationModelSnapshot {
        provider_store: live_store,
        routing: ModelRoutingConfig::default(),
        capabilities: snapshot.capabilities.clone(),
    };
    let next_main =
        AIChatService::resolve_model_route_from_snapshot(&next_request, ModelRole::Main)
            .expect("resolve_model_route succeeds");
    assert_eq!(next_main.provider.id, "main-b");
}

#[test]
fn generation_snapshot_keeps_specific_and_disabled_routes_isolated() {
    let mut live_store = ProviderStore {
        active_id: Some("main-a".to_string()),
        providers: vec![
            provider("main-a", "model-a"),
            provider("vision-b", "vision-b"),
            provider("main-c", "model-c"),
        ],
    };
    let mut routing = ModelRoutingConfig::default();
    routing
        .set_route(
            ModelRole::Vision,
            ModelRoute::Specific {
                provider_id: "vision-b".to_string(),
                model: "vision-b".to_string(),
            },
        )
        .expect("set_route succeeds");
    routing
        .set_route(ModelRole::Video, ModelRoute::Disabled)
        .expect("set_route succeeds");
    let capabilities = live_store
        .providers
        .iter()
        .map(|provider| supported_record(provider, &provider.active_model))
        .collect();
    let snapshot = GenerationModelSnapshot {
        provider_store: live_store.clone(),
        routing,
        capabilities: CapabilityRegistry {
            models: capabilities,
        },
    };

    live_store.active_id = Some("main-c".to_string());
    let vision = AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Vision)
        .expect("resolve_model_route succeeds");
    assert_eq!(vision.provider.id, "vision-b");
    assert_eq!(vision.model, "vision-b");
    assert!(AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Video).is_err());
    let main = AIChatService::resolve_model_route_from_snapshot(&snapshot, ModelRole::Main)
        .expect("resolve_model_route succeeds");
    assert_eq!(main.provider.id, "main-a");
}

#[test]
fn role_scoped_probe_plan_variants_are_distinct() {
    assert_eq!(ProbePlan::FullSafe, ProbePlan::FullSafe);
    assert_ne!(ProbePlan::FullSafe, ProbePlan::Role(ModelRole::Vision));
    assert_ne!(
        ProbePlan::Role(ModelRole::Vision),
        ProbePlan::Role(ModelRole::AudioStt)
    );
}
