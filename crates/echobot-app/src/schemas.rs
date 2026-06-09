//! Request and response types for the EchoBot HTTP API.
//!
//! All types are `Serialize + Deserialize` and use `serde(rename_all = "camelCase")`
//! to match the Python FastAPI shapes exactly. Newline-delimited JSON
//! streaming responses are emitted as raw `serde_json::Value` so the
//! browser can parse them incrementally.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// Summary entry returned by `GET /api/sessions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummaryModel {
    pub name: String,
    pub message_count: usize,
    pub updated_at: String,
}

/// Full session detail returned by `GET /api/sessions/current` and the
/// `PUT /api/sessions/{name}`-style mutators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetailModel {
    pub name: String,
    pub updated_at: String,
    #[serde(default)]
    pub compressed_summary: String,
    pub role_name: String,
    pub route_mode: String,
    #[serde(default)]
    pub history: Vec<MessageModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageModel {
    pub role: String,
    pub content: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallModel {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetCurrentSessionRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameSessionRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetSessionRoleRequest {
    pub role_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetSessionRouteModeRequest {
    pub route_mode: String,
}

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatImageInput {
    pub attachment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatFileInput {
    pub attachment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub prompt: String,
    #[serde(default = "default_session_name")]
    pub session_name: String,
    #[serde(default)]
    pub role_name: Option<String>,
    #[serde(default)]
    pub route_mode: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub images: Vec<ChatImageInput>,
    #[serde(default)]
    pub files: Vec<ChatFileInput>,
}

fn default_session_name() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub session_name: String,
    pub response: String,
    #[serde(default)]
    pub response_content: serde_json::Value,
    pub updated_at: String,
    pub steps: usize,
    #[serde(default)]
    pub compressed_summary: String,
    #[serde(default)]
    pub delegated: bool,
    #[serde(default = "default_true")]
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_role_name")]
    pub role_name: String,
}

fn default_true() -> bool {
    true
}

fn default_status() -> String {
    "completed".to_string()
}

fn default_role_name() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatJobSummaryModel {
    pub job_id: String,
    pub session_name: String,
    pub prompt: String,
    pub role_name: String,
    pub status: String,
    #[serde(default = "default_attempt")]
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of_job_id: Option<String>,
    #[serde(default)]
    pub can_retry: bool,
    #[serde(default)]
    pub error: String,
    pub created_at: String,
    pub started_at: String,
    #[serde(default)]
    pub finished_at: String,
    pub updated_at: String,
}

fn default_attempt() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatJobsResponse {
    #[serde(default)]
    pub jobs: Vec<ChatJobSummaryModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatJobResponse {
    pub job_id: String,
    pub session_name: String,
    pub prompt: String,
    pub role_name: String,
    pub status: String,
    #[serde(default = "default_attempt")]
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of_job_id: Option<String>,
    #[serde(default)]
    pub can_retry: bool,
    #[serde(default)]
    pub response: String,
    #[serde(default)]
    pub response_content: serde_json::Value,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub steps: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_user_input: Option<serde_json::Value>,
    pub created_at: String,
    pub started_at: String,
    #[serde(default)]
    pub finished_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatJobTraceResponse {
    pub job_id: String,
    pub session_name: String,
    pub status: String,
    pub updated_at: String,
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Cron
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronStatusResponse {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub jobs: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobModel {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub schedule: String,
    #[serde(default = "default_payload_kind")]
    pub payload_kind: String,
    #[serde(default = "default_session_name")]
    pub session_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

fn default_payload_kind() -> String {
    "agent".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobsResponse {
    #[serde(default)]
    pub jobs: Vec<CronJobModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronDeleteResponse {
    pub deleted: bool,
    pub job_id: String,
}

// ---------------------------------------------------------------------------
// Heartbeat
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfigResponse {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub interval_seconds: u64,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub has_meaningful_content: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateHeartbeatRequest {
    #[serde(default)]
    pub content: String,
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleSummaryModel {
    pub name: String,
    #[serde(default = "default_true")]
    pub editable: bool,
    #[serde(default = "default_true")]
    pub deletable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDetailModel {
    pub name: String,
    #[serde(default = "default_true")]
    pub editable: bool,
    #[serde(default = "default_true")]
    pub deletable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default)]
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoleRequest {
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRoleRequest {
    pub prompt: String,
}

// ---------------------------------------------------------------------------
// Attachments
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachmentResponse {
    pub attachment_id: String,
    pub url: String,
    pub preview_url: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub original_filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAttachmentResponse {
    pub attachment_id: String,
    pub url: String,
    pub download_url: String,
    pub content_type: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub original_filename: String,
    pub workspace_path: String,
}

// ---------------------------------------------------------------------------
// Web / TTS / ASR
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TTSRequest {
    pub text: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub rate: Option<String>,
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default)]
    pub pitch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TTSVoiceModel {
    pub name: String,
    pub short_name: String,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub gender: String,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TTSVoicesResponse {
    pub provider: String,
    #[serde(default)]
    pub voices: Vec<TTSVoiceModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebTTSProviderModel {
    pub name: String,
    pub label: String,
    #[serde(default = "default_true")]
    pub available: bool,
    #[serde(default = "default_ready_state")]
    pub state: String,
    #[serde(default)]
    pub detail: String,
}

fn default_ready_state() -> String {
    "ready".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebTTSConfigModel {
    #[serde(default = "default_tts_provider")]
    pub default_provider: String,
    #[serde(default)]
    pub default_voice: String,
    #[serde(default)]
    pub default_voices: HashMap<String, String>,
    #[serde(default)]
    pub providers: Vec<WebTTSProviderModel>,
}

fn default_tts_provider() -> String {
    "edge".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSpeechProviderModel {
    #[serde(default = "default_asr_kind")]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub available: bool,
    #[serde(default = "default_missing_state")]
    pub state: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub resource_directory: String,
}

fn default_asr_kind() -> String {
    "asr".to_string()
}

fn default_missing_state() -> String {
    "missing".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebASRConfigModel {
    #[serde(default)]
    pub available: bool,
    #[serde(default = "default_missing_state")]
    pub state: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default)]
    pub selected_asr_provider: String,
    #[serde(default)]
    pub selected_vad_provider: String,
    #[serde(default)]
    pub always_listen_supported: bool,
    #[serde(default)]
    pub asr_providers: Vec<WebSpeechProviderModel>,
    #[serde(default)]
    pub vad_providers: Vec<WebSpeechProviderModel>,
}

fn default_sample_rate() -> u32 {
    16_000
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateWebASRProviderRequest {
    #[serde(default)]
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebLive2DExpressionModel {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebLive2DMotionModel {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebLive2DHotkeyModel {
    #[serde(default)]
    pub hotkey_key: String,
    #[serde(default)]
    pub hotkey_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub shortcut_tokens: Vec<String>,
    #[serde(default)]
    pub shortcut_label: String,
    #[serde(default)]
    pub target_kind: String,
    #[serde(default)]
    pub supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateWebLive2DAnnotationRequest {
    #[serde(default)]
    pub selection_key: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebLive2DAnnotationResponse {
    #[serde(default)]
    pub selection_key: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateWebLive2DHotkeyRequest {
    #[serde(default)]
    pub selection_key: String,
    #[serde(default)]
    pub hotkey_key: String,
    #[serde(default)]
    pub shortcut_tokens: Vec<String>,
    #[serde(default)]
    pub restore_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebLive2DHotkeyResponse {
    #[serde(default)]
    pub selection_key: String,
    #[serde(default)]
    pub hotkey_key: String,
    #[serde(default)]
    pub hotkey_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub shortcut_tokens: Vec<String>,
    #[serde(default)]
    pub shortcut_label: String,
    #[serde(default)]
    pub target_kind: String,
    #[serde(default)]
    pub supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebLive2DModelOptionModel {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub selection_key: String,
    #[serde(default)]
    pub model_name: String,
    #[serde(default)]
    pub model_url: String,
    #[serde(default)]
    pub directory_name: String,
    #[serde(default)]
    pub lip_sync_parameter_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouth_form_parameter_id: Option<String>,
    #[serde(default)]
    pub expressions: Vec<WebLive2DExpressionModel>,
    #[serde(default)]
    pub motions: Vec<WebLive2DMotionModel>,
    #[serde(default)]
    pub hotkeys: Vec<WebLive2DHotkeyModel>,
    #[serde(default)]
    pub annotations_writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebLive2DConfigModel {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub models: Vec<WebLive2DModelOptionModel>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub selection_key: String,
    #[serde(default)]
    pub model_name: String,
    #[serde(default)]
    pub model_url: String,
    #[serde(default)]
    pub directory_name: String,
    #[serde(default)]
    pub lip_sync_parameter_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouth_form_parameter_id: Option<String>,
    #[serde(default)]
    pub expressions: Vec<WebLive2DExpressionModel>,
    #[serde(default)]
    pub motions: Vec<WebLive2DMotionModel>,
    #[serde(default)]
    pub hotkeys: Vec<WebLive2DHotkeyModel>,
    #[serde(default)]
    pub annotations_writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebStageBackgroundModel {
    #[serde(default = "default_background_key")]
    pub key: String,
    #[serde(default = "default_background_label")]
    pub label: String,
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_background_kind")]
    pub kind: String,
}

fn default_background_key() -> String {
    "default".to_string()
}

fn default_background_label() -> String {
    "不使用背景".to_string()
}

fn default_background_kind() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebStageConfigModel {
    #[serde(default = "default_background_key")]
    pub default_background_key: String,
    #[serde(default)]
    pub backgrounds: Vec<WebStageBackgroundModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebRuntimeConfigModel {
    #[serde(default = "default_true")]
    pub delegated_ack_enabled: bool,
    #[serde(default = "default_safety_mode")]
    pub shell_safety_mode: String,
    #[serde(default = "default_true")]
    pub file_write_enabled: bool,
    #[serde(default = "default_true")]
    pub cron_mutation_enabled: bool,
    #[serde(default)]
    pub web_private_network_enabled: bool,
}

fn default_safety_mode() -> String {
    "danger-full-access".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebConfigResponse {
    #[serde(default = "default_session_name")]
    pub session_name: String,
    #[serde(default = "default_role_name")]
    pub role_name: String,
    #[serde(default = "default_route_mode")]
    pub route_mode: String,
    #[serde(default)]
    pub runtime: WebRuntimeConfigModel,
    #[serde(default)]
    pub live2d: WebLive2DConfigModel,
    #[serde(default)]
    pub stage: WebStageConfigModel,
    #[serde(default)]
    pub asr: WebASRConfigModel,
    #[serde(default)]
    pub tts: WebTTSConfigModel,
}

fn default_route_mode() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateWebRuntimeConfigRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_ack_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_safety_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_write_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_mutation_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_private_network_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ASRTranscriptionResponse {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub language: String,
}
