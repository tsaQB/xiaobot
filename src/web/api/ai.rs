//! AI page: providers, the main model, specialist routes, capability
//! corrections and model tests. Changes apply to the running daemon at once.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::Json;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ai::routing::{ModelRole, ModelRoute, ResolvedModelRoute};
use crate::ai::storage::{
    load_provider_store, save_provider_store, CapabilityEvidence, CapabilityEvidenceSource,
    CapabilityKind, CapabilityRecord, CapabilityState, ProbeEvent, ProbeOutcome, ProviderConfig,
    ProviderStore,
};
use crate::web::error::{ApiError, ApiResult};
use crate::web::settings::{self, SecretMeta};
use crate::web::WebState;

use super::ok;

const ROLES: [(&str, ModelRole); 6] = [
    ("main", ModelRole::Main),
    ("vision", ModelRole::Vision),
    ("video", ModelRole::Video),
    ("audio_stt", ModelRole::AudioStt),
    ("image_gen", ModelRole::ImageGeneration),
    ("curator", ModelRole::Curator),
];

const CAPS: [(&str, CapabilityKind); 11] = [
    ("text_chat", CapabilityKind::TextChat),
    ("tools", CapabilityKind::Tools),
    ("reasoning", CapabilityKind::Reasoning),
    ("image_input", CapabilityKind::ImageInput),
    ("video_input", CapabilityKind::VideoInput),
    ("audio_input", CapabilityKind::AudioInput),
    ("audio_transcription", CapabilityKind::AudioTranscription),
    ("image_generation", CapabilityKind::ImageGeneration),
    ("image_editing", CapabilityKind::ImageEditing),
    ("structured_output", CapabilityKind::StructuredOutput),
    ("native_file_input", CapabilityKind::NativeFileInput),
];

const IMAGE_KEYS: [&str; 5] = [
    "IMAGE_FALLBACK_PROVIDER",
    "IMAGE_GENERATION_TIMEOUT_SECS",
    "IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS",
    "IMAGE_DOWNLOAD_TIMEOUT_SECS",
    "AI_PROVIDER_CONNECT_TIMEOUT_SECS",
];

fn role_from_path(raw: &str) -> Result<(&'static str, ModelRole), ApiError> {
    ModelRole::parse(raw)
        .and_then(|role| ROLES.iter().find(|(_, known)| *known == role).copied())
        .ok_or_else(|| {
            ApiError::invalid(
                format!("Unknown role '{raw}'"),
                format!("Peran '{raw}' tidak dikenal"),
            )
        })
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Every capability of a record with its state, deciding evidence and override.
fn cap_map(record: Option<&CapabilityRecord>) -> Value {
    let mut map = serde_json::Map::new();
    for (key, kind) in CAPS {
        let state = record.map_or(CapabilityState::Unknown, |record| {
            record.effective_state_for(kind)
        });
        let source = record
            .and_then(|record| record.effective_evidence_for(kind))
            .map(|evidence| to_json(&evidence.source));
        let correction = record
            .and_then(|record| {
                record.evidence.iter().find(|evidence| {
                    evidence.capability == kind
                        && evidence.source == CapabilityEvidenceSource::UserOverride
                })
            })
            .map(|evidence| {
                if evidence.outcome == CapabilityState::Supported {
                    "yes"
                } else {
                    "no"
                }
            });
        map.insert(
            key.to_string(),
            json!({"state": to_json(&state), "source": source, "override": correction}),
        );
    }
    Value::Object(map)
}

fn active_id(store: &ProviderStore) -> Option<String> {
    store
        .active_id
        .clone()
        .filter(|id| store.providers.iter().any(|provider| &provider.id == id))
        .or_else(|| store.providers.first().map(|provider| provider.id.clone()))
}

fn provider_view(provider: &ProviderConfig, active: Option<&str>) -> Value {
    json!({
        "id": provider.id,
        "name": provider.name,
        "endpoint": provider.endpoint,
        "models": provider.models,
        "active_model": provider.active_model,
        "active": active == Some(provider.id.as_str()),
        "key": SecretMeta::stored(&provider.api_key, true),
    })
}

async fn load_store() -> Result<ProviderStore, ApiError> {
    tokio::task::spawn_blocking(load_provider_store)
        .await
        .map_err(ApiError::internal)
}

/// Saves the provider store and makes the daemon use it.
async fn save_store(state: &WebState, store: ProviderStore) -> Result<(), ApiError> {
    tokio::task::spawn_blocking(move || save_provider_store(&store))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    if !state.ai.reload_provider_store().await {
        return Err(ApiError::internal("provider store could not be reloaded"));
    }
    Ok(())
}

fn normalize_endpoint(raw: &str) -> Result<String, ApiError> {
    crate::cli::wizard::normalize_endpoint_url(raw).map_err(|error| {
        ApiError::invalid(
            format!("Endpoint: {error}"),
            format!("Endpoint tidak valid: {error}"),
        )
    })
}

fn clean_key(raw: &str) -> String {
    let key = raw.trim().trim_matches(['"', '\'', '`']).to_string();
    if key.is_empty() {
        "none".to_string()
    } else {
        key
    }
}

fn number_setting(key: &str) -> u64 {
    settings::effective(key).trim().parse().unwrap_or(0)
}

/// GET /api/ai
pub(crate) async fn state(State(state): State<Arc<WebState>>) -> ApiResult<Value> {
    let store = state.ai.provider_store.read().await.clone();
    let active = active_id(&store);
    let providers: Vec<Value> = store
        .providers
        .iter()
        .map(|provider| provider_view(provider, active.as_deref()))
        .collect();
    let routing = state.ai.model_routing_config().await;
    let mut roles = Vec::new();
    for (id, role) in ROLES {
        let route = match routing.route(role) {
            Some(route) => to_json(route),
            None => {
                let main = active
                    .as_deref()
                    .and_then(|id| store.providers.iter().find(|provider| provider.id == id));
                json!({
                    "type": "specific",
                    "provider_id": main.map(|provider| provider.id.clone()),
                    "model": main.map(|provider| provider.active_model.clone()),
                })
            }
        };
        let view = match state.ai.resolve_model_route(role).await {
            Ok(resolved) => {
                let record = state
                    .ai
                    .capability_record(&resolved.provider.endpoint, &resolved.model)
                    .await;
                json!({
                    "id": id,
                    "route": route,
                    "provider": resolved.provider.name,
                    "model": resolved.model,
                    "error": null,
                    "caps": cap_map(record.as_ref()),
                    "checked_at": record
                        .map(|record| record.checked_at)
                        .filter(|checked| !checked.is_empty()),
                })
            }
            Err(error) => json!({
                "id": id,
                "route": route,
                "provider": null,
                "model": null,
                "error": error,
                "caps": cap_map(None),
                "checked_at": null,
            }),
        };
        roles.push(view);
    }
    let fallback =
        if settings::effective("IMAGE_FALLBACK_PROVIDER").eq_ignore_ascii_case("pollinations") {
            "pollinations"
        } else {
            "none"
        };
    Ok(Json(json!({
        "providers": providers,
        "roles": roles,
        "image": {
            "fallback": fallback,
            "gen_timeout": number_setting("IMAGE_GENERATION_TIMEOUT_SECS"),
            "connect_timeout": number_setting("IMAGE_PROVIDER_CONNECT_TIMEOUT_SECS"),
            "download_timeout": number_setting("IMAGE_DOWNLOAD_TIMEOUT_SECS"),
        },
        "ai_limits": {"connect_timeout": number_setting("AI_PROVIDER_CONNECT_TIMEOUT_SECS")},
        "env_locks": settings::env_locks(IMAGE_KEYS),
        "restart_needed": !state.restart_keys().is_empty(),
    })))
}

#[derive(Deserialize)]
pub(crate) struct ProviderTestRequest {
    endpoint: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
}

/// POST /api/ai/providers/test
pub(crate) async fn test_provider(
    State(state): State<Arc<WebState>>,
    Json(body): Json<ProviderTestRequest>,
) -> ApiResult<Value> {
    let endpoint = normalize_endpoint(&body.endpoint)?;
    let typed = body.api_key.unwrap_or_default();
    let key = if typed.trim().is_empty() {
        match body.provider_id {
            Some(id) => load_store()
                .await?
                .providers
                .into_iter()
                .find(|provider| provider.id == id)
                .map(|provider| provider.api_key)
                .ok_or_else(ApiError::not_found)?,
            None => "none".to_string(),
        }
    } else {
        clean_key(&typed)
    };
    let started = Instant::now();
    let (_, result) = state.ai.fetch_models_from_endpoint(&endpoint, &key).await;
    let ms = started.elapsed().as_millis();
    Ok(Json(match result {
        Ok(models) => {
            json!({"ok": true, "ms": ms, "endpoint": endpoint, "models": models, "error": null})
        }
        Err(error) => {
            json!({"ok": false, "ms": ms, "endpoint": endpoint, "models": [], "error": error})
        }
    }))
}

#[derive(Deserialize)]
pub(crate) struct ProviderCreateRequest {
    name: String,
    endpoint: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    model: Option<String>,
}

fn default_name(endpoint: &str) -> String {
    if endpoint.contains("openrouter.ai") {
        "OpenRouter".to_string()
    } else {
        url::Url::parse(endpoint)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_else(|| "Custom Provider".to_string())
    }
}

/// POST /api/ai/providers
pub(crate) async fn create_provider(
    State(state): State<Arc<WebState>>,
    Json(body): Json<ProviderCreateRequest>,
) -> ApiResult<Value> {
    let endpoint = normalize_endpoint(&body.endpoint)?;
    let api_key = clean_key(&body.api_key);
    let models = state
        .ai
        .fetch_models_from_endpoint(&endpoint, &api_key)
        .await
        .1
        .map_err(ApiError::upstream)?;
    let active_model = body
        .model
        .filter(|model| models.contains(model))
        .or_else(|| models.first().cloned())
        .unwrap_or_else(|| "default".to_string());
    let suffix: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(6)
        .map(char::from)
        .collect();
    let name = body.name.trim();
    let provider = ProviderConfig {
        id: format!("prov_{}", suffix.to_lowercase()),
        name: if name.is_empty() {
            default_name(&endpoint)
        } else {
            crate::util::truncate_chars(name, 60)
        },
        endpoint,
        api_key,
        api_key_ref: None,
        models,
        active_model,
    };
    let mut store = load_store().await?;
    if active_id(&store).is_none() {
        store.active_id = Some(provider.id.clone());
    }
    store.providers.push(provider.clone());
    let active = active_id(&store);
    save_store(&state, store).await?;
    Ok(Json(provider_view(&provider, active.as_deref())))
}

#[derive(Deserialize)]
pub(crate) struct ProviderUpdateRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

/// PUT /api/ai/providers/:id (an empty key keeps the stored one)
pub(crate) async fn update_provider(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
    Json(body): Json<ProviderUpdateRequest>,
) -> ApiResult<Value> {
    let mut store = load_store().await?;
    let active = active_id(&store);
    let provider = store
        .providers
        .iter_mut()
        .find(|provider| provider.id == id)
        .ok_or_else(ApiError::not_found)?;
    if let Some(name) = body
        .name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
    {
        provider.name = crate::util::truncate_chars(&name, 60);
    }
    let mut reconnect = false;
    if let Some(endpoint) = body.endpoint.filter(|endpoint| !endpoint.trim().is_empty()) {
        let endpoint = normalize_endpoint(&endpoint)?;
        reconnect |= endpoint != provider.endpoint;
        provider.endpoint = endpoint;
    }
    if let Some(key) = body.api_key.filter(|key| !key.trim().is_empty()) {
        provider.api_key = clean_key(&key);
        reconnect = true;
    }
    if reconnect {
        let models = state
            .ai
            .fetch_models_from_endpoint(&provider.endpoint, &provider.api_key)
            .await
            .1
            .map_err(ApiError::upstream)?;
        provider.models = models;
    }
    let view = provider_view(provider, active.as_deref());
    save_store(&state, store).await?;
    Ok(Json(view))
}

/// DELETE /api/ai/providers/:id
pub(crate) async fn delete_provider(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    let mut store = load_store().await?;
    if !store.providers.iter().any(|provider| provider.id == id) {
        return Err(ApiError::not_found());
    }
    if active_id(&store).as_deref() == Some(id.as_str()) {
        return Err(ApiError::conflict(
            "The active provider cannot be removed. Pick another main model first.",
            "Provider aktif tidak bisa dihapus. Pilih model utama lain dulu.",
        ));
    }
    let dependencies = state.ai.provider_route_dependencies(&id).await;
    if !dependencies.is_empty() {
        let names = dependencies
            .iter()
            .map(|role| role.display_name())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(ApiError::conflict(
            format!("Still used by: {names}. Change those routes first."),
            format!("Masih dipakai oleh: {names}. Ubah rute itu dulu."),
        ));
    }
    store.providers.retain(|provider| provider.id != id);
    save_store(&state, store).await?;
    Ok(ok())
}

/// POST /api/ai/providers/:id/models
pub(crate) async fn refresh_models(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    let mut store = load_store().await?;
    let active = active_id(&store);
    let provider = store
        .providers
        .iter_mut()
        .find(|provider| provider.id == id)
        .ok_or_else(ApiError::not_found)?;
    let models = state
        .ai
        .fetch_models_from_endpoint(&provider.endpoint, &provider.api_key)
        .await
        .1
        .map_err(ApiError::upstream)?;
    let added = models
        .iter()
        .filter(|model| !provider.models.contains(model))
        .count();
    let removed = provider
        .models
        .iter()
        .filter(|model| !models.contains(model))
        .count();
    provider.models = models;
    let view = provider_view(provider, active.as_deref());
    let count = provider.models.len();
    save_store(&state, store).await?;
    Ok(Json(json!({
        "ok": true,
        "models": count,
        "added": added,
        "removed": removed,
        "provider": view,
    })))
}

#[derive(Deserialize)]
pub(crate) struct ActiveModelRequest {
    provider_id: String,
    model: String,
}

/// POST /api/ai/active
pub(crate) async fn set_active(
    State(state): State<Arc<WebState>>,
    Json(body): Json<ActiveModelRequest>,
) -> ApiResult<Value> {
    let model = body.model.trim().to_string();
    if model.is_empty() {
        return Err(ApiError::invalid("Pick a model.", "Pilih model."));
    }
    let mut store = load_store().await?;
    let provider = store
        .providers
        .iter_mut()
        .find(|provider| provider.id == body.provider_id)
        .ok_or_else(ApiError::not_found)?;
    if !provider.models.is_empty() && !provider.models.contains(&model) {
        return Err(ApiError::invalid(
            format!("'{model}' is not in this provider's catalogue. Refresh it first."),
            format!("'{model}' tidak ada di katalog provider ini. Ambil ulang katalognya dulu."),
        ));
    }
    provider.active_model = model;
    store.active_id = Some(body.provider_id);
    save_store(&state, store).await?;
    Ok(ok())
}

/// PUT /api/ai/routes/:role
pub(crate) async fn set_route(
    State(state): State<Arc<WebState>>,
    Path(role): Path<String>,
    Json(route): Json<ModelRoute>,
) -> ApiResult<Value> {
    let (_, role) = role_from_path(&role)?;
    state
        .ai
        .set_model_route(role, route)
        .await
        .map_err(|error| ApiError::invalid(error.clone(), error))?;
    Ok(ok())
}

#[derive(Deserialize)]
pub(crate) struct CapOverrideRequest {
    overrides: HashMap<String, String>,
}

/// PUT /api/ai/caps/:role: manual corrections, which win over probes.
pub(crate) async fn set_caps(
    State(state): State<Arc<WebState>>,
    Path(role): Path<String>,
    Json(body): Json<CapOverrideRequest>,
) -> ApiResult<Value> {
    let (_, role) = role_from_path(&role)?;
    let route = state
        .ai
        .resolve_model_route(role)
        .await
        .map_err(|error| ApiError::conflict(error.clone(), error))?;
    let provider_id = route.provider.endpoint.trim_end_matches('/').to_string();
    let now = chrono::Local::now().to_rfc3339();
    let mut candidate = state.ai.capability_registry.read().await.clone();
    let index = match candidate
        .models
        .iter()
        .position(|record| record.provider_id == provider_id && record.model == route.model)
    {
        Some(index) => index,
        None => {
            candidate.models.push(CapabilityRecord {
                provider_id: provider_id.clone(),
                provider_name: route.provider.name.clone(),
                model: route.model.clone(),
                ..CapabilityRecord::default()
            });
            candidate.models.len() - 1
        }
    };
    let Some(record) = candidate.models.get_mut(index) else {
        return Err(ApiError::internal("capability record missing"));
    };
    for (key, choice) in &body.overrides {
        let Some((_, kind)) = CAPS.iter().find(|(name, _)| name == key).copied() else {
            return Err(ApiError::invalid(
                format!("Unknown capability '{key}'"),
                format!("Kemampuan '{key}' tidak dikenal"),
            ));
        };
        let outcome = match choice.as_str() {
            "auto" => None,
            "yes" => Some(CapabilityState::Supported),
            "no" => Some(CapabilityState::Unsupported),
            _ => {
                return Err(ApiError::invalid(
                    "Each correction is auto, yes or no.",
                    "Setiap koreksi harus auto, yes, atau no.",
                ))
            }
        };
        record.evidence.retain(|evidence| {
            evidence.capability != kind || evidence.source != CapabilityEvidenceSource::UserOverride
        });
        if let Some(outcome) = outcome {
            record.evidence.push(CapabilityEvidence {
                capability: kind,
                source: CapabilityEvidenceSource::UserOverride,
                outcome,
                checked_at: now.clone(),
                detail: Some("corrected in the WebUI".to_string()),
            });
        }
    }
    if !crate::ai::storage::persist_capability_registry(candidate.clone()).await {
        return Err(ApiError::internal("capability registry could not be saved"));
    }
    *state.ai.capability_registry.write().await = candidate;
    Ok(ok())
}

fn describe_probe_event(event: &ProbeEvent) -> String {
    match event {
        ProbeEvent::Started { capability } => format!("{}: started", to_json(capability)),
        ProbeEvent::Progress { message, .. } => message.clone(),
        ProbeEvent::Completed {
            capability,
            outcome,
        } => format!("{}: {}", to_json(capability), to_json(outcome)),
        ProbeEvent::Skipped { capability, reason } => {
            format!("{}: skipped ({reason})", to_json(capability))
        }
        ProbeEvent::Persistence { saved } => format!("saved: {saved}"),
        ProbeEvent::Finished => "finished".to_string(),
    }
}

fn outcome_of(state: CapabilityState) -> ProbeOutcome {
    match state {
        CapabilityState::Supported => ProbeOutcome::Supported,
        CapabilityState::Unsupported => ProbeOutcome::Unsupported,
        CapabilityState::Unknown => ProbeOutcome::Inconclusive,
    }
}

/// POST /api/ai/probe/:role
pub(crate) async fn probe(
    State(state): State<Arc<WebState>>,
    Path(role): Path<String>,
) -> ApiResult<Value> {
    let (id, role) = role_from_path(&role)?;
    let mut log = Vec::new();
    let mut saved = false;
    let observer = |event: ProbeEvent| {
        if let ProbeEvent::Persistence { saved: stored } = &event {
            saved = *stored;
        }
        log.push(describe_probe_event(&event));
    };
    let (record, outcome, model) = if role == ModelRole::Main {
        let route = state
            .ai
            .resolve_model_route(ModelRole::Main)
            .await
            .map_err(|error| ApiError::conflict(error.clone(), error))?;
        let record = state
            .ai
            .probe_model_capabilities_with_observer(&route.provider, &route.model, observer)
            .await;
        let outcome = outcome_of(record.effective_state_for(CapabilityKind::TextChat));
        (record, outcome, route.model)
    } else {
        let (record, outcome) = state
            .ai
            .probe_addon_role_with_observer(role, observer)
            .await
            .map_err(|error| ApiError::conflict(error.clone(), error))?;
        let model = record.model.clone();
        (record, outcome, model)
    };
    Ok(Json(json!({
        "role": id,
        "model": model,
        "outcome": to_json(&outcome),
        "saved": saved,
        "caps": cap_map(Some(&record)),
        "checked_at": Some(record.checked_at.clone()).filter(|checked| !checked.is_empty()),
        "log": log,
    })))
}

/// A short real chat request through a resolved route.
async fn chat_ping(state: &WebState, route: &ResolvedModelRoute) -> Result<String, String> {
    let url = crate::ai::service::provider_url(&route.provider.endpoint, "chat/completions");
    let mut request = state
        .ai
        .client
        .post(url)
        .timeout(Duration::from_secs(90))
        .json(&json!({
            "model": route.model,
            "messages": [{"role": "user", "content": "Reply with the single word OK."}],
            "max_tokens": 512,
            "stream": false,
        }));
    let key = route.provider.api_key.trim();
    if !key.is_empty() && !["none", "-", "no", "null"].contains(&key.to_ascii_lowercase().as_str())
    {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| error.without_url().to_string())?;
    let status = response.status();
    let body = crate::ai::service::read_bounded_json(response).await?;
    if !status.is_success() {
        let message = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("request failed");
        return Err(format!(
            "HTTP {}: {}",
            status.as_u16(),
            crate::util::truncate_chars(message, 200)
        ));
    }
    let reply = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    Ok(crate::util::truncate_chars(reply, 200))
}

/// POST /api/ai/test/:role
pub(crate) async fn test_role(
    State(state): State<Arc<WebState>>,
    Path(role): Path<String>,
) -> ApiResult<Value> {
    let (id, role) = role_from_path(&role)?;
    let started = Instant::now();
    let (ok, model, detail) = match role {
        ModelRole::Main | ModelRole::Curator => {
            let route = state
                .ai
                .resolve_model_route(role)
                .await
                .map_err(|error| ApiError::conflict(error.clone(), error))?;
            match chat_ping(&state, &route).await {
                Ok(reply) => (true, Some(route.model), reply),
                Err(error) => (false, Some(route.model), error),
            }
        }
        ModelRole::ImageGeneration => {
            match state
                .ai
                .probe_image_generation_active_with_observer(role, |_| {})
                .await
            {
                Ok((record, outcome)) => (
                    outcome == ProbeOutcome::Supported,
                    Some(record.model),
                    to_json(&outcome).as_str().unwrap_or_default().to_string(),
                ),
                Err(error) => (false, None, error),
            }
        }
        _ => match state.ai.probe_addon_role_with_observer(role, |_| {}).await {
            Ok((record, outcome)) => (
                outcome == ProbeOutcome::Supported,
                Some(record.model),
                to_json(&outcome).as_str().unwrap_or_default().to_string(),
            ),
            Err(error) => (false, None, error),
        },
    };
    Ok(Json(json!({
        "role": id,
        "ok": ok,
        "ms": started.elapsed().as_millis(),
        "model": model,
        "detail": detail,
    })))
}
