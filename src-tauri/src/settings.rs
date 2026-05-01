use log::{debug, warn};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use specta::Type;
use std::collections::HashMap;
use std::fmt;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

pub const APPLE_INTELLIGENCE_PROVIDER_ID: &str = "apple_intelligence";
pub const APPLE_INTELLIGENCE_DEFAULT_MODEL_ID: &str = "Apple Intelligence";

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

// Custom deserializer to handle both old numeric format (1-5) and new string format ("trace", "debug", etc.)
impl<'de> Deserialize<'de> for LogLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct LogLevelVisitor;

        impl<'de> Visitor<'de> for LogLevelVisitor {
            type Value = LogLevel;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or integer representing log level")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<LogLevel, E> {
                match value.to_lowercase().as_str() {
                    "trace" => Ok(LogLevel::Trace),
                    "debug" => Ok(LogLevel::Debug),
                    "info" => Ok(LogLevel::Info),
                    "warn" => Ok(LogLevel::Warn),
                    "error" => Ok(LogLevel::Error),
                    _ => Err(E::unknown_variant(
                        value,
                        &["trace", "debug", "info", "warn", "error"],
                    )),
                }
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<LogLevel, E> {
                match value {
                    1 => Ok(LogLevel::Trace),
                    2 => Ok(LogLevel::Debug),
                    3 => Ok(LogLevel::Info),
                    4 => Ok(LogLevel::Warn),
                    5 => Ok(LogLevel::Error),
                    _ => Err(E::invalid_value(de::Unexpected::Unsigned(value), &"1-5")),
                }
            }
        }

        deserializer.deserialize_any(LogLevelVisitor)
    }
}

impl From<LogLevel> for tauri_plugin_log::LogLevel {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Trace => tauri_plugin_log::LogLevel::Trace,
            LogLevel::Debug => tauri_plugin_log::LogLevel::Debug,
            LogLevel::Info => tauri_plugin_log::LogLevel::Info,
            LogLevel::Warn => tauri_plugin_log::LogLevel::Warn,
            LogLevel::Error => tauri_plugin_log::LogLevel::Error,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct ShortcutBinding {
    pub id: String,
    pub name: String,
    pub description: String,
    pub default_binding: String,
    pub current_binding: String,
}

/// A chord configuration for selecting an `LLMPrompt` via tap-count.
///
/// `tap_count` is the number of times the user taps the transcribe shortcut
/// before holding/recording. `1` is implicitly plain transcription (no preset);
/// `2` = double-tap, `3` = triple-tap, etc.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct PresetChord {
    pub tap_count: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct LLMPrompt {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// Chord assignment for this preset. `None` means the preset is configured
    /// but not currently bound to any tap-count. Persisted settings written
    /// before the chord migration won't have this field; serde defaults it.
    #[serde(default)]
    pub chord: Option<PresetChord>,
}

/// How the API key is sent to the LLM provider.
///
/// Most OpenAI-compatible APIs use `Authorization: Bearer <key>`. Anthropic
/// uses `x-api-key: <key>`. Some custom gateways (e.g. AWS API Gateway with
/// API-key auth, or vendor-specific) require `x-api-key`. Each provider's
/// auth scheme is fixed by its vendor, except `custom` where the user picks
/// to match whatever their gateway expects.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    BearerToken,
    XApiKey,
}

impl Default for AuthMethod {
    fn default() -> Self {
        AuthMethod::BearerToken
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct PostProcessProvider {
    pub id: String,
    pub label: String,
    pub base_url: String,
    #[serde(default)]
    pub allow_base_url_edit: bool,
    #[serde(default)]
    pub models_endpoint: Option<String>,
    #[serde(default)]
    pub supports_structured_output: bool,
    #[serde(default)]
    pub auth_method: AuthMethod,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPosition {
    None,
    Top,
    Bottom,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum ModelUnloadTimeout {
    Never,
    Immediately,
    Min2,
    Min5,
    Min10,
    Min15,
    Hour1,
    Sec15, // Debug mode only
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PasteMethod {
    CtrlV,
    Direct,
    None,
    ShiftInsert,
    CtrlShiftV,
    ExternalScript,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardHandling {
    DontModify,
    CopyToClipboard,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum AutoSubmitKey {
    Enter,
    CtrlEnter,
    CmdEnter,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum RecordingRetentionPeriod {
    Never,
    PreserveLimit,
    Days3,
    Weeks2,
    Months3,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardImplementation {
    Tauri,
    HandyKeys,
}

impl Default for KeyboardImplementation {
    fn default() -> Self {
        #[cfg(target_os = "linux")]
        return KeyboardImplementation::Tauri;
        #[cfg(not(target_os = "linux"))]
        return KeyboardImplementation::HandyKeys;
    }
}

impl Default for ModelUnloadTimeout {
    fn default() -> Self {
        ModelUnloadTimeout::Min5
    }
}

impl Default for PasteMethod {
    fn default() -> Self {
        // Default to CtrlV for macOS and Windows, Direct for Linux
        #[cfg(target_os = "linux")]
        return PasteMethod::Direct;
        #[cfg(not(target_os = "linux"))]
        return PasteMethod::CtrlV;
    }
}

impl Default for ClipboardHandling {
    fn default() -> Self {
        ClipboardHandling::DontModify
    }
}

impl Default for AutoSubmitKey {
    fn default() -> Self {
        AutoSubmitKey::Enter
    }
}

impl ModelUnloadTimeout {
    pub fn to_minutes(self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0), // Special case for immediate unloading
            ModelUnloadTimeout::Min2 => Some(2),
            ModelUnloadTimeout::Min5 => Some(5),
            ModelUnloadTimeout::Min10 => Some(10),
            ModelUnloadTimeout::Min15 => Some(15),
            ModelUnloadTimeout::Hour1 => Some(60),
            ModelUnloadTimeout::Sec15 => Some(0), // Special case for debug - handled separately
        }
    }

    pub fn to_seconds(self) -> Option<u64> {
        match self {
            ModelUnloadTimeout::Never => None,
            ModelUnloadTimeout::Immediately => Some(0), // Special case for immediate unloading
            ModelUnloadTimeout::Sec15 => Some(15),
            _ => self.to_minutes().map(|m| m * 60),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum SoundTheme {
    Marimba,
    Pop,
    Custom,
}

impl SoundTheme {
    fn as_str(&self) -> &'static str {
        match self {
            SoundTheme::Marimba => "marimba",
            SoundTheme::Pop => "pop",
            SoundTheme::Custom => "custom",
        }
    }

    pub fn to_start_path(&self) -> String {
        format!("resources/{}_start.wav", self.as_str())
    }

    pub fn to_stop_path(&self) -> String {
        format!("resources/{}_stop.wav", self.as_str())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum TypingTool {
    Auto,
    Wtype,
    Kwtype,
    Dotool,
    Ydotool,
    Xdotool,
}

impl Default for TypingTool {
    fn default() -> Self {
        TypingTool::Auto
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum WhisperAcceleratorSetting {
    Auto,
    Cpu,
    Gpu,
}

impl Default for WhisperAcceleratorSetting {
    fn default() -> Self {
        WhisperAcceleratorSetting::Auto
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum OrtAcceleratorSetting {
    Auto,
    Cpu,
    Cuda,
    #[serde(rename = "directml")]
    DirectMl,
    Rocm,
}

impl Default for OrtAcceleratorSetting {
    fn default() -> Self {
        OrtAcceleratorSetting::Auto
    }
}

#[derive(Clone, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub(crate) struct SecretMap(HashMap<String, String>);

impl fmt::Debug for SecretMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted: HashMap<&String, &str> = self
            .0
            .iter()
            .map(|(k, v)| (k, if v.is_empty() { "" } else { "[REDACTED]" }))
            .collect();
        redacted.fmt(f)
    }
}

impl std::ops::Deref for SecretMap {
    type Target = HashMap<String, String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for SecretMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/* still handy for composing the initial JSON in the store ------------- */
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct AppSettings {
    pub bindings: HashMap<String, ShortcutBinding>,
    pub push_to_talk: bool,
    pub audio_feedback: bool,
    #[serde(default = "default_audio_feedback_volume")]
    pub audio_feedback_volume: f32,
    #[serde(default = "default_sound_theme")]
    pub sound_theme: SoundTheme,
    #[serde(default = "default_start_hidden")]
    pub start_hidden: bool,
    #[serde(default = "default_autostart_enabled")]
    pub autostart_enabled: bool,
    #[serde(default = "default_update_checks_enabled")]
    pub update_checks_enabled: bool,
    #[serde(default = "default_model")]
    pub selected_model: String,
    #[serde(default = "default_always_on_microphone")]
    pub always_on_microphone: bool,
    #[serde(default)]
    pub selected_microphone: Option<String>,
    #[serde(default)]
    pub clamshell_microphone: Option<String>,
    #[serde(default)]
    pub selected_output_device: Option<String>,
    #[serde(default = "default_translate_to_english")]
    pub translate_to_english: bool,
    #[serde(default = "default_selected_language")]
    pub selected_language: String,
    #[serde(default = "default_overlay_position")]
    pub overlay_position: OverlayPosition,
    #[serde(default = "default_debug_mode")]
    pub debug_mode: bool,
    #[serde(default = "default_log_level")]
    pub log_level: LogLevel,
    #[serde(default)]
    pub custom_words: Vec<String>,
    #[serde(default)]
    pub model_unload_timeout: ModelUnloadTimeout,
    #[serde(default = "default_word_correction_threshold")]
    pub word_correction_threshold: f64,
    #[serde(default = "default_history_limit")]
    pub history_limit: usize,
    #[serde(default = "default_recording_retention_period")]
    pub recording_retention_period: RecordingRetentionPeriod,
    #[serde(default)]
    pub paste_method: PasteMethod,
    #[serde(default)]
    pub clipboard_handling: ClipboardHandling,
    #[serde(default = "default_auto_submit")]
    pub auto_submit: bool,
    #[serde(default)]
    pub auto_submit_key: AutoSubmitKey,
    #[serde(default = "default_post_process_enabled")]
    pub post_process_enabled: bool,
    #[serde(default = "default_post_process_provider_id")]
    pub post_process_provider_id: String,
    #[serde(default = "default_post_process_providers")]
    pub post_process_providers: Vec<PostProcessProvider>,
    #[serde(default = "default_post_process_api_keys")]
    pub post_process_api_keys: SecretMap,
    #[serde(default = "default_post_process_models")]
    pub post_process_models: HashMap<String, String>,
    #[serde(default = "default_post_process_prompts")]
    pub post_process_prompts: Vec<LLMPrompt>,
    #[serde(default)]
    pub post_process_selected_prompt_id: Option<String>,
    #[serde(default)]
    pub mute_while_recording: bool,
    #[serde(default)]
    pub append_trailing_space: bool,
    #[serde(default = "default_app_language")]
    pub app_language: String,
    #[serde(default)]
    pub experimental_enabled: bool,
    #[serde(default)]
    pub lazy_stream_close: bool,
    #[serde(default)]
    pub keyboard_implementation: KeyboardImplementation,
    #[serde(default = "default_show_tray_icon")]
    pub show_tray_icon: bool,
    #[serde(default = "default_paste_delay_ms")]
    pub paste_delay_ms: u64,
    #[serde(default = "default_typing_tool")]
    pub typing_tool: TypingTool,
    pub external_script_path: Option<String>,
    #[serde(default)]
    pub custom_filler_words: Option<Vec<String>>,
    #[serde(default)]
    pub whisper_accelerator: WhisperAcceleratorSetting,
    #[serde(default)]
    pub ort_accelerator: OrtAcceleratorSetting,
    #[serde(default = "default_whisper_gpu_device")]
    pub whisper_gpu_device: i32,
    #[serde(default)]
    pub extra_recording_buffer_ms: u64,
    /// Split recordings into chunks of this many seconds and transcribe each chunk
    /// sequentially with context carry-forward to avoid mid-sentence punctuation.
    /// `None` disables chunking (full audio transcribed at once).
    #[serde(default)]
    pub streaming_chunk_duration_s: Option<u32>,
}

fn default_model() -> String {
    "".to_string()
}

fn default_always_on_microphone() -> bool {
    false
}

fn default_translate_to_english() -> bool {
    false
}

fn default_start_hidden() -> bool {
    false
}

fn default_autostart_enabled() -> bool {
    false
}

fn default_update_checks_enabled() -> bool {
    true
}

fn default_selected_language() -> String {
    "auto".to_string()
}

fn default_overlay_position() -> OverlayPosition {
    #[cfg(target_os = "linux")]
    return OverlayPosition::None;
    #[cfg(not(target_os = "linux"))]
    return OverlayPosition::Bottom;
}

fn default_debug_mode() -> bool {
    false
}

fn default_log_level() -> LogLevel {
    LogLevel::Debug
}

fn default_word_correction_threshold() -> f64 {
    0.18
}

fn default_paste_delay_ms() -> u64 {
    60
}

fn default_auto_submit() -> bool {
    false
}

fn default_history_limit() -> usize {
    5
}

fn default_recording_retention_period() -> RecordingRetentionPeriod {
    RecordingRetentionPeriod::PreserveLimit
}

fn default_audio_feedback_volume() -> f32 {
    1.0
}

fn default_sound_theme() -> SoundTheme {
    SoundTheme::Marimba
}

fn default_post_process_enabled() -> bool {
    false
}

fn default_app_language() -> String {
    tauri_plugin_os::locale()
        .map(|l| l.replace('_', "-"))
        .unwrap_or_else(|| "en".to_string())
}

fn default_show_tray_icon() -> bool {
    true
}

fn default_post_process_provider_id() -> String {
    "openai".to_string()
}

fn default_post_process_providers() -> Vec<PostProcessProvider> {
    let mut providers = vec![
        PostProcessProvider {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: true,
            auth_method: AuthMethod::BearerToken,
        },
        PostProcessProvider {
            id: "zai".to_string(),
            label: "Z.AI".to_string(),
            base_url: "https://api.z.ai/api/paas/v4".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: true,
            auth_method: AuthMethod::BearerToken,
        },
        PostProcessProvider {
            id: "openrouter".to_string(),
            label: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: true,
            auth_method: AuthMethod::BearerToken,
        },
        PostProcessProvider {
            id: "anthropic".to_string(),
            label: "Anthropic".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: false,
            auth_method: AuthMethod::XApiKey,
        },
        PostProcessProvider {
            id: "groq".to_string(),
            label: "Groq".to_string(),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: false,
            auth_method: AuthMethod::BearerToken,
        },
        PostProcessProvider {
            id: "cerebras".to_string(),
            label: "Cerebras".to_string(),
            base_url: "https://api.cerebras.ai/v1".to_string(),
            allow_base_url_edit: false,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: true,
            auth_method: AuthMethod::BearerToken,
        },
    ];

    // Note: We always include Apple Intelligence on macOS ARM64 without checking availability
    // at startup. The availability check is deferred to when the user actually tries to use it
    // (in actions.rs). This prevents crashes on macOS 26.x beta where accessing
    // SystemLanguageModel.default during early app initialization causes SIGABRT.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        providers.push(PostProcessProvider {
            id: APPLE_INTELLIGENCE_PROVIDER_ID.to_string(),
            label: "Apple Intelligence".to_string(),
            base_url: "apple-intelligence://local".to_string(),
            allow_base_url_edit: false,
            models_endpoint: None,
            supports_structured_output: true,
            auth_method: AuthMethod::BearerToken,
        });
    }

    // AWS Bedrock via Mantle (OpenAI-compatible endpoint)
    providers.push(PostProcessProvider {
        id: "bedrock_mantle".to_string(),
        label: "AWS Bedrock (Mantle)".to_string(),
        base_url: "https://bedrock-mantle.us-east-1.api.aws/v1".to_string(),
        allow_base_url_edit: false,
        models_endpoint: Some("/models".to_string()),
        supports_structured_output: true,
        auth_method: AuthMethod::BearerToken,
    });

    // Custom provider always comes last
    providers.push(PostProcessProvider {
        id: "custom".to_string(),
        label: "Custom".to_string(),
        base_url: "http://localhost:11434/v1".to_string(),
        allow_base_url_edit: true,
        models_endpoint: Some("/models".to_string()),
        supports_structured_output: false,
        auth_method: AuthMethod::BearerToken,
    });

    providers
}

fn default_post_process_api_keys() -> SecretMap {
    let mut map = HashMap::new();
    for provider in default_post_process_providers() {
        map.insert(provider.id, String::new());
    }
    SecretMap(map)
}

fn default_model_for_provider(provider_id: &str) -> String {
    if provider_id == APPLE_INTELLIGENCE_PROVIDER_ID {
        return APPLE_INTELLIGENCE_DEFAULT_MODEL_ID.to_string();
    }
    String::new()
}

fn default_post_process_models() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for provider in default_post_process_providers() {
        map.insert(
            provider.id.clone(),
            default_model_for_provider(&provider.id),
        );
    }
    map
}

fn default_post_process_prompts() -> Vec<LLMPrompt> {
    vec![LLMPrompt {
        id: "default_improve_transcriptions".to_string(),
        name: "Improve Transcriptions".to_string(),
        prompt: "Clean this transcript:\n1. Fix spelling, capitalization, and punctuation errors\n2. Convert number words to digits (twenty-five → 25, ten percent → 10%, five dollars → $5)\n3. Replace spoken punctuation with symbols (period → ., comma → ,, question mark → ?)\n4. Remove filler words (um, uh, like as filler)\n5. Keep the language in the original version (if it was french, keep it in french for example)\n\nPreserve exact meaning and word order. Do not paraphrase or reorder content.\n\nReturn only the cleaned transcript.\n\nTranscript:\n${output}".to_string(),
        chord: None,
    }]
}

fn default_whisper_gpu_device() -> i32 {
    -1 // auto
}

fn default_typing_tool() -> TypingTool {
    TypingTool::Auto
}

fn ensure_post_process_defaults(settings: &mut AppSettings) -> bool {
    let mut changed = false;
    for provider in default_post_process_providers() {
        // Use match to do a single lookup - either sync existing or add new
        match settings
            .post_process_providers
            .iter_mut()
            .find(|p| p.id == provider.id)
        {
            Some(existing) => {
                // Sync supports_structured_output field for existing providers (migration)
                if existing.supports_structured_output != provider.supports_structured_output {
                    debug!(
                        "Updating supports_structured_output for provider '{}' from {} to {}",
                        provider.id,
                        existing.supports_structured_output,
                        provider.supports_structured_output
                    );
                    existing.supports_structured_output = provider.supports_structured_output;
                    changed = true;
                }
                // Sync auth_method from defaults for vendor-fixed providers.
                // The custom provider is the only one whose auth_method is
                // user-configurable (UI exposes it), so we never override it.
                if provider.id != "custom" && existing.auth_method != provider.auth_method {
                    debug!(
                        "Updating auth_method for provider '{}' from {:?} to {:?}",
                        provider.id, existing.auth_method, provider.auth_method
                    );
                    existing.auth_method = provider.auth_method;
                    changed = true;
                }
            }
            None => {
                // Provider doesn't exist, add it
                settings.post_process_providers.push(provider.clone());
                changed = true;
            }
        }

        if !settings.post_process_api_keys.contains_key(&provider.id) {
            settings
                .post_process_api_keys
                .insert(provider.id.clone(), String::new());
            changed = true;
        }

        let default_model = default_model_for_provider(&provider.id);
        match settings.post_process_models.get_mut(&provider.id) {
            Some(existing) => {
                if existing.is_empty() && !default_model.is_empty() {
                    *existing = default_model.clone();
                    changed = true;
                }
            }
            None => {
                settings
                    .post_process_models
                    .insert(provider.id.clone(), default_model);
                changed = true;
            }
        }
    }

    changed
}

/// Migrate the legacy `transcribe_with_post_process` model to the chord system.
///
/// Two changes:
///   1. The user's previously selected post-process prompt
///      (`post_process_selected_prompt_id`) is assigned a double-tap chord —
///      preserving its behavior under the new model. After consuming it the
///      field is cleared, which also serves as the migration's idempotency
///      marker (re-running on already-migrated settings is a no-op).
///   2. The legacy `transcribe_with_post_process` shortcut binding is dropped.
///
/// Returns `true` when settings were modified (caller should persist).
fn migrate_chord_v1(settings: &mut AppSettings) -> bool {
    let mut changed = false;

    if let Some(prompt_id) = settings.post_process_selected_prompt_id.take() {
        changed = true;
        if let Some(prompt) = settings
            .post_process_prompts
            .iter_mut()
            .find(|p| p.id == prompt_id)
        {
            if prompt.chord.is_none() {
                prompt.chord = Some(PresetChord { tap_count: 2 });
            }
        }
    }

    if settings
        .bindings
        .remove("transcribe_with_post_process")
        .is_some()
    {
        changed = true;
    }

    changed
}

pub const SETTINGS_STORE_PATH: &str = "settings_store.json";

pub fn get_default_settings() -> AppSettings {
    #[cfg(target_os = "windows")]
    let default_shortcut = "ctrl+space";
    #[cfg(target_os = "macos")]
    let default_shortcut = "option+space";
    #[cfg(target_os = "linux")]
    let default_shortcut = "ctrl+space";
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    let default_shortcut = "alt+space";

    let mut bindings = HashMap::new();
    bindings.insert(
        "transcribe".to_string(),
        ShortcutBinding {
            id: "transcribe".to_string(),
            name: "Transcribe".to_string(),
            description: "Converts your speech into text.".to_string(),
            default_binding: default_shortcut.to_string(),
            current_binding: default_shortcut.to_string(),
        },
    );
    // The legacy `transcribe_with_post_process` binding has been removed —
    // post-processing is now reached by tap-count chord on the regular
    // `transcribe` shortcut (e.g. double-tap → run the chord-bound preset).
    // `migrate_chord_v1` drops the binding from existing user settings on load.
    bindings.insert(
        "cancel".to_string(),
        ShortcutBinding {
            id: "cancel".to_string(),
            name: "Cancel".to_string(),
            description: "Cancels the current recording.".to_string(),
            default_binding: "escape".to_string(),
            current_binding: "escape".to_string(),
        },
    );

    AppSettings {
        bindings,
        push_to_talk: true,
        audio_feedback: false,
        audio_feedback_volume: default_audio_feedback_volume(),
        sound_theme: default_sound_theme(),
        start_hidden: default_start_hidden(),
        autostart_enabled: default_autostart_enabled(),
        update_checks_enabled: default_update_checks_enabled(),
        selected_model: "".to_string(),
        always_on_microphone: false,
        selected_microphone: None,
        clamshell_microphone: None,
        selected_output_device: None,
        translate_to_english: false,
        selected_language: "auto".to_string(),
        overlay_position: default_overlay_position(),
        debug_mode: false,
        log_level: default_log_level(),
        custom_words: Vec::new(),
        model_unload_timeout: ModelUnloadTimeout::default(),
        word_correction_threshold: default_word_correction_threshold(),
        history_limit: default_history_limit(),
        recording_retention_period: default_recording_retention_period(),
        paste_method: PasteMethod::default(),
        clipboard_handling: ClipboardHandling::default(),
        auto_submit: default_auto_submit(),
        auto_submit_key: AutoSubmitKey::default(),
        post_process_enabled: default_post_process_enabled(),
        post_process_provider_id: default_post_process_provider_id(),
        post_process_providers: default_post_process_providers(),
        post_process_api_keys: default_post_process_api_keys(),
        post_process_models: default_post_process_models(),
        post_process_prompts: default_post_process_prompts(),
        post_process_selected_prompt_id: None,
        mute_while_recording: false,
        append_trailing_space: false,
        app_language: default_app_language(),
        experimental_enabled: false,
        lazy_stream_close: false,
        keyboard_implementation: KeyboardImplementation::default(),
        show_tray_icon: default_show_tray_icon(),
        paste_delay_ms: default_paste_delay_ms(),
        typing_tool: default_typing_tool(),
        external_script_path: None,
        custom_filler_words: None,
        whisper_accelerator: WhisperAcceleratorSetting::default(),
        ort_accelerator: OrtAcceleratorSetting::default(),
        whisper_gpu_device: default_whisper_gpu_device(),
        extra_recording_buffer_ms: 0,
        streaming_chunk_duration_s: None,
    }
}

impl AppSettings {
    pub fn active_post_process_provider(&self) -> Option<&PostProcessProvider> {
        self.post_process_providers
            .iter()
            .find(|provider| provider.id == self.post_process_provider_id)
    }

    pub fn post_process_provider(&self, provider_id: &str) -> Option<&PostProcessProvider> {
        self.post_process_providers
            .iter()
            .find(|provider| provider.id == provider_id)
    }

    pub fn post_process_provider_mut(
        &mut self,
        provider_id: &str,
    ) -> Option<&mut PostProcessProvider> {
        self.post_process_providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
    }

    /// Resolve a tap-count to the prompt id it should invoke.
    ///
    /// `count == 0` and `count == 1` always return `None` (plain transcription;
    /// no LLM post-processing). For `count >= 2`, returns the id of the first
    /// prompt whose `chord.tap_count` matches.
    pub fn preset_id_for_chord_count(&self, count: u32) -> Option<String> {
        if count < 2 {
            return None;
        }
        self.post_process_prompts
            .iter()
            .find(|p| p.chord.as_ref().map(|c| c.tap_count) == Some(count))
            .map(|p| p.id.clone())
    }

    /// Check whether assigning `count` to the prompt with `id` would collide
    /// with another prompt's existing chord. Returns the id of the conflicting
    /// prompt (or `None` if no conflict).
    ///
    /// Counts below 2 are treated as "no chord requested" and never conflict.
    /// A prompt does not conflict with itself.
    pub fn chord_conflict(&self, id: &str, count: u32) -> Option<String> {
        if count < 2 {
            return None;
        }
        self.post_process_prompts
            .iter()
            .find(|p| p.id != id && p.chord.as_ref().map(|c| c.tap_count) == Some(count))
            .map(|p| p.id.clone())
    }
}

pub fn load_or_create_app_settings(app: &AppHandle) -> AppSettings {
    // Initialize store
    let store = app
        .store(crate::portable::store_path(SETTINGS_STORE_PATH))
        .expect("Failed to initialize store");

    let mut settings = if let Some(settings_value) = store.get("settings") {
        // Parse the entire settings object
        match serde_json::from_value::<AppSettings>(settings_value) {
            Ok(mut settings) => {
                // Avoid logging the full struct — its `Debug` impl redacts
                // API keys today, but logging a summary instead removes the
                // ongoing dependency on that redaction holding as the
                // codebase evolves.
                debug!(
                    "Loaded existing settings: {} bindings, {} providers, {} prompts",
                    settings.bindings.len(),
                    settings.post_process_providers.len(),
                    settings.post_process_prompts.len(),
                );
                let default_settings = get_default_settings();
                let mut updated = false;

                // Merge default bindings into existing settings
                for (key, value) in default_settings.bindings {
                    if !settings.bindings.contains_key(&key) {
                        debug!("Adding missing binding: {}", key);
                        settings.bindings.insert(key, value);
                        updated = true;
                    }
                }

                if updated {
                    debug!("Settings updated with new bindings");
                    store.set("settings", serde_json::to_value(&settings).unwrap());
                }

                settings
            }
            Err(e) => {
                warn!("Failed to parse settings: {}", e);
                // Fall back to default settings if parsing fails
                let default_settings = get_default_settings();
                store.set("settings", serde_json::to_value(&default_settings).unwrap());
                default_settings
            }
        }
    } else {
        let default_settings = get_default_settings();
        store.set("settings", serde_json::to_value(&default_settings).unwrap());
        default_settings
    };

    let mut changed = ensure_post_process_defaults(&mut settings);
    changed |= migrate_chord_v1(&mut settings);
    if changed {
        store.set("settings", serde_json::to_value(&settings).unwrap());
    }

    settings
}

pub fn get_settings(app: &AppHandle) -> AppSettings {
    let store = app
        .store(crate::portable::store_path(SETTINGS_STORE_PATH))
        .expect("Failed to initialize store");

    let mut settings = if let Some(settings_value) = store.get("settings") {
        serde_json::from_value::<AppSettings>(settings_value).unwrap_or_else(|_| {
            let default_settings = get_default_settings();
            store.set("settings", serde_json::to_value(&default_settings).unwrap());
            default_settings
        })
    } else {
        let default_settings = get_default_settings();
        store.set("settings", serde_json::to_value(&default_settings).unwrap());
        default_settings
    };

    let mut changed = ensure_post_process_defaults(&mut settings);
    changed |= migrate_chord_v1(&mut settings);
    if changed {
        store.set("settings", serde_json::to_value(&settings).unwrap());
    }

    settings
}

pub fn write_settings(app: &AppHandle, settings: AppSettings) {
    let store = app
        .store(crate::portable::store_path(SETTINGS_STORE_PATH))
        .expect("Failed to initialize store");

    store.set("settings", serde_json::to_value(&settings).unwrap());
}

pub fn get_bindings(app: &AppHandle) -> HashMap<String, ShortcutBinding> {
    let settings = get_settings(app);

    settings.bindings
}

pub fn get_stored_binding(app: &AppHandle, id: &str) -> ShortcutBinding {
    let bindings = get_bindings(app);

    let binding = bindings.get(id).unwrap().clone();

    binding
}

pub fn get_history_limit(app: &AppHandle) -> usize {
    let settings = get_settings(app);
    settings.history_limit
}

pub fn get_recording_retention_period(app: &AppHandle) -> RecordingRetentionPeriod {
    let settings = get_settings(app);
    settings.recording_retention_period
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_disable_auto_submit() {
        let settings = get_default_settings();
        assert!(!settings.auto_submit);
        assert_eq!(settings.auto_submit_key, AutoSubmitKey::Enter);
    }

    #[test]
    fn debug_output_redacts_api_keys() {
        let mut settings = get_default_settings();
        settings
            .post_process_api_keys
            .insert("openai".to_string(), "sk-proj-secret-key-12345".to_string());
        settings.post_process_api_keys.insert(
            "anthropic".to_string(),
            "sk-ant-secret-key-67890".to_string(),
        );
        settings
            .post_process_api_keys
            .insert("empty_provider".to_string(), "".to_string());

        let debug_output = format!("{:?}", settings);

        assert!(!debug_output.contains("sk-proj-secret-key-12345"));
        assert!(!debug_output.contains("sk-ant-secret-key-67890"));
        assert!(debug_output.contains("[REDACTED]"));
    }

    #[test]
    fn secret_map_debug_redacts_values() {
        let map = SecretMap(HashMap::from([("key".into(), "secret".into())]));
        let out = format!("{:?}", map);
        assert!(!out.contains("secret"));
        assert!(out.contains("[REDACTED]"));
    }

    // ---------------------------------------------------------------------
    // Chord-system migration tests (Phase 1 of chord-system.md).
    //
    // The migration replaces the legacy "transcribe_with_post_process"
    // binding model with per-prompt tap-count chords. On upgrade:
    //   - The user's previously selected post-process prompt gets
    //     `chord = Some(PresetChord { tap_count: 2 })`.
    //   - `post_process_selected_prompt_id` is cleared (its job is now
    //     done by the chord on the prompt itself).
    //   - The legacy binding is dropped from `settings.bindings`.
    // ---------------------------------------------------------------------

    fn prompt(id: &str, chord: Option<PresetChord>) -> LLMPrompt {
        LLMPrompt {
            id: id.to_string(),
            name: format!("Prompt {id}"),
            prompt: "test".to_string(),
            chord,
        }
    }

    #[test]
    fn chord_migration_assigns_double_tap_to_selected_prompt() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![prompt("p1", None)];
        settings.post_process_selected_prompt_id = Some("p1".to_string());

        let changed = migrate_chord_v1(&mut settings);

        assert!(changed, "first run should report a change");
        assert_eq!(
            settings.post_process_prompts[0].chord,
            Some(PresetChord { tap_count: 2 }),
            "selected prompt should be assigned double-tap chord"
        );
    }

    #[test]
    fn chord_migration_drops_legacy_post_process_binding() {
        let mut settings = get_default_settings();
        // Defaults no longer contain the legacy binding (the chord system
        // replaced it). Re-insert it as if loaded from a pre-migration store
        // so we can verify the migration removes it.
        settings.bindings.insert(
            "transcribe_with_post_process".to_string(),
            ShortcutBinding {
                id: "transcribe_with_post_process".to_string(),
                name: "Legacy".into(),
                description: "Legacy".into(),
                default_binding: "ctrl+shift+space".into(),
                current_binding: "ctrl+shift+space".into(),
            },
        );

        migrate_chord_v1(&mut settings);

        assert!(
            !settings
                .bindings
                .contains_key("transcribe_with_post_process"),
            "legacy binding should be removed by migration"
        );
        assert!(
            settings.bindings.contains_key("transcribe"),
            "primary transcribe binding must be preserved"
        );
    }

    #[test]
    fn default_settings_have_no_legacy_post_process_binding() {
        let settings = get_default_settings();
        assert!(
            !settings
                .bindings
                .contains_key("transcribe_with_post_process"),
            "legacy binding must not appear in defaults — chord-system replaces it"
        );
        assert!(settings.bindings.contains_key("transcribe"));
        assert!(settings.bindings.contains_key("cancel"));
    }

    #[test]
    fn chord_migration_clears_selected_prompt_id() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![prompt("p1", None)];
        settings.post_process_selected_prompt_id = Some("p1".to_string());

        migrate_chord_v1(&mut settings);

        assert_eq!(
            settings.post_process_selected_prompt_id, None,
            "selected_prompt_id is the migration's signal; it should be cleared after"
        );
    }

    #[test]
    fn chord_migration_is_idempotent() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![prompt("p1", None)];
        settings.post_process_selected_prompt_id = Some("p1".to_string());

        let first = migrate_chord_v1(&mut settings);
        let snapshot_chord = settings.post_process_prompts[0].chord.clone();
        let snapshot_selected = settings.post_process_selected_prompt_id.clone();
        let snapshot_has_legacy = settings
            .bindings
            .contains_key("transcribe_with_post_process");

        let second = migrate_chord_v1(&mut settings);

        assert!(first, "first run reports change");
        assert!(!second, "second run is a no-op");
        assert_eq!(snapshot_chord, settings.post_process_prompts[0].chord);
        assert_eq!(snapshot_selected, settings.post_process_selected_prompt_id);
        assert_eq!(
            snapshot_has_legacy,
            settings
                .bindings
                .contains_key("transcribe_with_post_process")
        );
    }

    #[test]
    fn chord_migration_preserves_existing_chord() {
        let mut settings = get_default_settings();
        let existing = PresetChord { tap_count: 5 };
        settings.post_process_prompts = vec![prompt("p1", Some(existing.clone()))];
        settings.post_process_selected_prompt_id = Some("p1".to_string());

        migrate_chord_v1(&mut settings);

        assert_eq!(
            settings.post_process_prompts[0].chord,
            Some(existing),
            "migration must not overwrite a chord the user (or a prior migration) already set"
        );
    }

    #[test]
    fn chord_migration_handles_unknown_selected_prompt_id() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![]; // selected_prompt_id refers to nothing
        settings.post_process_selected_prompt_id = Some("does_not_exist".to_string());

        // Must not panic, must still consume the orphan selected_prompt_id.
        let changed = migrate_chord_v1(&mut settings);

        assert!(changed);
        assert_eq!(settings.post_process_selected_prompt_id, None);
    }

    #[test]
    fn chord_migration_no_op_when_already_clean() {
        let mut settings = get_default_settings();
        settings.post_process_selected_prompt_id = None;
        settings.bindings.remove("transcribe_with_post_process");

        let changed = migrate_chord_v1(&mut settings);

        assert!(!changed, "nothing to migrate ⇒ no change reported");
    }

    #[test]
    fn llm_prompt_default_has_no_chord() {
        let prompts = default_post_process_prompts();
        assert!(
            !prompts.is_empty(),
            "default prompts list must be populated"
        );
        assert!(
            prompts.iter().all(|p| p.chord.is_none()),
            "default prompts ship with no chord assigned"
        );
    }

    // ---- preset_id_for_chord_count ----------------------------------------

    #[test]
    fn preset_lookup_returns_none_for_count_below_2() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![prompt("p1", Some(PresetChord { tap_count: 1 }))]; // tap_count:1 is malformed but defensively shouldn't be returned
        assert_eq!(settings.preset_id_for_chord_count(0), None);
        assert_eq!(settings.preset_id_for_chord_count(1), None);
    }

    #[test]
    fn preset_lookup_finds_matching_chord() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![
            prompt("p1", Some(PresetChord { tap_count: 2 })),
            prompt("p2", Some(PresetChord { tap_count: 3 })),
            prompt("p3", None),
        ];
        assert_eq!(
            settings.preset_id_for_chord_count(2),
            Some("p1".to_string())
        );
        assert_eq!(
            settings.preset_id_for_chord_count(3),
            Some("p2".to_string())
        );
    }

    #[test]
    fn preset_lookup_returns_none_for_unconfigured_count() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![prompt("p1", Some(PresetChord { tap_count: 2 }))];
        assert_eq!(settings.preset_id_for_chord_count(4), None);
    }

    #[test]
    fn preset_lookup_returns_first_match_when_two_prompts_collide() {
        // Settings UI is supposed to prevent this, but defend against bad state.
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![
            prompt("p_first", Some(PresetChord { tap_count: 2 })),
            prompt("p_second", Some(PresetChord { tap_count: 2 })),
        ];
        assert_eq!(
            settings.preset_id_for_chord_count(2),
            Some("p_first".to_string())
        );
    }

    // ---- chord_conflict ----------------------------------------------------

    #[test]
    fn chord_conflict_returns_none_when_no_other_prompt_has_count() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![
            prompt("p1", Some(PresetChord { tap_count: 2 })),
            prompt("p2", None),
        ];
        // Asking for count=3 — nobody owns it.
        assert_eq!(settings.chord_conflict("p1", 3), None);
    }

    #[test]
    fn chord_conflict_finds_existing_owner() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![
            prompt("p1", Some(PresetChord { tap_count: 2 })),
            prompt("p2", None),
        ];
        // p2 wants count=2 which p1 already owns.
        assert_eq!(settings.chord_conflict("p2", 2), Some("p1".to_string()));
    }

    #[test]
    fn chord_conflict_ignores_self() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![prompt("p1", Some(PresetChord { tap_count: 2 }))];
        // p1 setting itself to its own current count is not a conflict.
        assert_eq!(settings.chord_conflict("p1", 2), None);
    }

    // ---- AuthMethod / provider defaults -------------------------------------

    #[test]
    fn default_anthropic_provider_uses_x_api_key() {
        let providers = default_post_process_providers();
        let anthropic = providers
            .iter()
            .find(|p| p.id == "anthropic")
            .expect("anthropic provider in defaults");
        assert_eq!(anthropic.auth_method, AuthMethod::XApiKey);
    }

    #[test]
    fn default_openai_provider_uses_bearer_token() {
        let providers = default_post_process_providers();
        let openai = providers
            .iter()
            .find(|p| p.id == "openai")
            .expect("openai provider in defaults");
        assert_eq!(openai.auth_method, AuthMethod::BearerToken);
    }

    #[test]
    fn default_custom_provider_uses_bearer_token() {
        let providers = default_post_process_providers();
        let custom = providers
            .iter()
            .find(|p| p.id == "custom")
            .expect("custom provider in defaults");
        assert_eq!(custom.auth_method, AuthMethod::BearerToken);
    }

    #[test]
    fn ensure_defaults_patches_legacy_anthropic_to_x_api_key() {
        // Existing user upgraded from a build where auth_method didn't exist —
        // anthropic deserialized with serde-default BearerToken, which is wrong.
        // ensure_post_process_defaults must correct it.
        let mut settings = get_default_settings();
        let anthropic = settings.post_process_provider_mut("anthropic").unwrap();
        anthropic.auth_method = AuthMethod::BearerToken;

        let changed = ensure_post_process_defaults(&mut settings);

        assert!(changed, "should report change when patching anthropic");
        assert_eq!(
            settings
                .post_process_provider("anthropic")
                .unwrap()
                .auth_method,
            AuthMethod::XApiKey,
        );
    }

    #[test]
    fn ensure_defaults_preserves_user_choice_on_custom_provider() {
        // The custom provider is the only one whose auth_method is user-
        // configurable via UI. ensure_post_process_defaults must not stomp
        // on whatever the user picked there.
        let mut settings = get_default_settings();
        let custom = settings.post_process_provider_mut("custom").unwrap();
        custom.auth_method = AuthMethod::XApiKey;

        ensure_post_process_defaults(&mut settings);

        assert_eq!(
            settings
                .post_process_provider("custom")
                .unwrap()
                .auth_method,
            AuthMethod::XApiKey,
            "user's custom auth choice must survive settings load"
        );
    }

    #[test]
    fn provider_deserializes_legacy_json_without_auth_method() {
        let json = r#"{
            "id": "openai",
            "label": "OpenAI",
            "base_url": "https://api.openai.com/v1"
        }"#;
        let p: PostProcessProvider = serde_json::from_str(json).expect("legacy compat");
        // serde default = BearerToken; ensure_post_process_defaults will correct
        // anthropic but openai is fine.
        assert_eq!(p.auth_method, AuthMethod::BearerToken);
    }

    #[test]
    fn chord_conflict_count_below_2_is_never_a_conflict() {
        // count < 2 means "plain" / "unbound" — we should not detect conflicts
        // even if a malformed prior config has another prompt with tap_count: 0/1.
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![prompt("p1", Some(PresetChord { tap_count: 1 }))];
        assert_eq!(settings.chord_conflict("p2", 0), None);
        assert_eq!(settings.chord_conflict("p2", 1), None);
    }

    #[test]
    fn preset_lookup_skips_prompts_without_chord() {
        let mut settings = get_default_settings();
        settings.post_process_prompts = vec![
            prompt("nochord", None),
            prompt("withchord", Some(PresetChord { tap_count: 2 })),
        ];
        assert_eq!(
            settings.preset_id_for_chord_count(2),
            Some("withchord".to_string())
        );
    }

    #[test]
    fn llm_prompt_deserializes_old_json_without_chord_field() {
        // Settings persisted before this migration won't contain a `chord` key.
        let json = r#"{
            "id": "x",
            "name": "X",
            "prompt": "p"
        }"#;
        let p: LLMPrompt = serde_json::from_str(json).expect("backwards-compat deserialize");
        assert_eq!(p.chord, None);
    }
}
