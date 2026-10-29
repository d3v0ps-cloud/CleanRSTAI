//! Voice input and reading replies aloud. Recording and playback run on their own threads (see
//! `record` and `speak`); the HTTP calls run on the tokio runtime. Results come back as
//! `AppEvent`s, so nothing here blocks the UI.
//!
//! Voice input records a clip, sends it as WAV to `POST {base}/audio/transcriptions` (OpenAI,
//! Groq, or a local server such as whisper.cpp, faster-whisper-server, speaches or LocalAI), and
//! puts the text in the message box. Reading aloud uses the OS voice, or
//! `POST {base}/audio/speech` for WAV audio.

pub mod record;
pub mod speak;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::{AppEvent, EventSender};
use crate::config::{ProviderConfig, Settings};
use crate::llm::{authorize, endpoint, error_from_response};
use crate::types::{ChatMessage, Role};

pub const DEFAULT_TRANSCRIBE_MODEL: &str = "whisper-1";
pub const DEFAULT_SPEECH_MODEL: &str = "tts-1";
pub const DEFAULT_SPEECH_VOICE: &str = "alloy";
/// Longest clip a recording may run, in seconds.
pub const MAX_RECORD_SECS: u32 = 600;
pub const MAX_SPEAK_CHARS: u32 = 20_000;

/// The reply being read aloud: its chat and message index.
pub type SpeechKey = (Uuid, usize);

/// Reports a background task's result exactly once. If the task panics before it reports, the
/// result is an error instead, so the UI never waits for an answer that is not coming.
pub struct Reporter<T, F: FnOnce(Result<T, String>)> {
    done: Option<F>,
    lost: &'static str,
    result: std::marker::PhantomData<fn(Result<T, String>)>,
}

impl<T, F: FnOnce(Result<T, String>)> Reporter<T, F> {
    pub fn new(lost: &'static str, done: F) -> Self {
        Self {
            done: Some(done),
            lost,
            result: std::marker::PhantomData,
        }
    }

    pub fn finish(mut self, result: Result<T, String>) {
        if let Some(done) = self.done.take() {
            done(result);
        }
    }
}

impl<T, F: FnOnce(Result<T, String>)> Drop for Reporter<T, F> {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            done(Err(self.lost.to_owned()));
        }
    }
}

/// Settings → Voice: voice input (Ctrl/⌘+Shift+M) and reading replies aloud (Ctrl/⌘+Shift+S).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VoiceSettings {
    pub input: VoiceInput,
    pub output: VoiceOutput,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VoiceInput {
    /// Show the microphone button and the shortcut.
    pub enabled: bool,
    /// The provider profile whose base URL, key and headers the transcription call uses. None
    /// means the active provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The transcription model, e.g. `whisper-1`, or a model name the local server serves.
    pub model: String,
    /// An ISO-639-1 code such as `de`; empty lets the model detect the language.
    pub language: String,
    /// A recording stops by itself after this many seconds.
    pub max_secs: u32,
}

impl Default for VoiceInput {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: None,
            model: DEFAULT_TRANSCRIBE_MODEL.into(),
            language: String::new(),
            max_secs: 120,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpeechEngine {
    /// The system's voice: `say` on macOS, System.Speech on Windows, spd-say or espeak on Linux.
    #[default]
    OsVoice,
    /// A text-to-speech endpoint (`POST {base}/audio/speech`).
    Endpoint,
}

impl SpeechEngine {
    pub fn label(self) -> &'static str {
        match self {
            Self::OsVoice => "System voice",
            Self::Endpoint => "Speech endpoint",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VoiceOutput {
    /// Show the speaker button under replies and the shortcut.
    pub enabled: bool,
    pub engine: SpeechEngine,
    /// The provider profile for the speech endpoint (ignored for the system voice). None means
    /// the active provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub model: String,
    pub voice: String,
    /// 0.5 to 2.0, 1.0 is normal speed.
    pub speed: f32,
    /// Replies longer than this are cut short before they are read.
    pub max_chars: u32,
}

impl Default for VoiceOutput {
    fn default() -> Self {
        Self {
            enabled: false,
            engine: SpeechEngine::OsVoice,
            provider: None,
            model: DEFAULT_SPEECH_MODEL.into(),
            voice: DEFAULT_SPEECH_VOICE.into(),
            speed: 1.0,
            max_chars: 4000,
        }
    }
}

impl VoiceSettings {
    /// Keeps the settings usable: blank models and voices fall back to the defaults, ranges are
    /// clamped, and a provider that no longer exists means the active provider.
    pub fn normalize(&mut self, providers: &[ProviderConfig]) {
        let known = |id: &mut Option<String>| {
            *id = id
                .take()
                .map(|s| s.trim().to_owned())
                .filter(|s| providers.iter().any(|p| &p.id == s));
        };
        known(&mut self.input.provider);
        known(&mut self.output.provider);
        self.input.model = non_blank(&self.input.model, DEFAULT_TRANSCRIBE_MODEL);
        self.input.language = self.input.language.trim().to_owned();
        self.input.max_secs = self.input.max_secs.clamp(5, MAX_RECORD_SECS);
        self.output.model = non_blank(&self.output.model, DEFAULT_SPEECH_MODEL);
        self.output.voice = non_blank(&self.output.voice, DEFAULT_SPEECH_VOICE);
        self.output.speed = if self.output.speed.is_finite() {
            self.output.speed.clamp(0.5, 2.0)
        } else {
            1.0
        };
        self.output.max_chars = self.output.max_chars.clamp(200, MAX_SPEAK_CHARS);
    }
}

fn non_blank(value: &str, default: &str) -> String {
    let v = value.trim();
    let out = if v.is_empty() { default } else { v };
    out.to_owned()
}

/// The provider a voice call uses: the chosen profile, else the active one.
fn voice_provider(settings: &Settings, id: Option<&str>) -> ProviderConfig {
    id.and_then(|id| settings.providers.iter().find(|p| p.id == id))
        .unwrap_or_else(|| settings.provider())
        .clone()
}

/// The provider for transcription, with the voice model in place of the chat model.
pub fn input_provider(settings: &Settings) -> ProviderConfig {
    let v = &settings.general.voice.input;
    let mut p = voice_provider(settings, v.provider.as_deref());
    p.model = non_blank(&v.model, DEFAULT_TRANSCRIBE_MODEL);
    p
}

/// The provider for the speech endpoint, with the speech model in place of the chat model.
pub fn output_provider(settings: &Settings) -> ProviderConfig {
    let v = &settings.general.voice.output;
    let mut p = voice_provider(settings, v.provider.as_deref());
    p.model = non_blank(&v.model, DEFAULT_SPEECH_MODEL);
    p
}

/// Puts a transcript into the message box: at the cursor when there is one (a char index),
/// otherwise at the end. A word that would touch another word gets a space between them.
pub fn insert_text(input: &mut String, cursor: Option<usize>, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    match cursor {
        Some(at) => {
            let at = at.min(input.chars().count());
            let byte = input.char_indices().nth(at).map_or(input.len(), |(b, _)| b);
            let before = &input[..byte];
            let after = input[byte..].to_owned();
            let mut out = String::with_capacity(input.len() + text.len() + 2);
            out.push_str(before);
            if before.chars().last().is_some_and(char::is_alphanumeric) {
                out.push(' ');
            }
            out.push_str(text);
            if after.chars().next().is_some_and(char::is_alphanumeric) {
                out.push(' ');
            }
            out.push_str(&after);
            *input = out;
        }
        None => {
            if input.chars().last().is_some_and(|c| !c.is_whitespace()) {
                input.push(' ');
            }
            input.push_str(text);
        }
    }
}

/// `POST {base}/audio/transcriptions`: the clip as a multipart `file`, with the `model` and,
/// when set, the `language`. Returns the `text` of the answer.
pub async fn transcribe(
    http: &reqwest::Client,
    p: &ProviderConfig,
    language: &str,
    wav: Vec<u8>,
) -> Result<String> {
    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("speech.wav")
        .mime_str("audio/wav")?;
    let mut form = reqwest::multipart::Form::new()
        .text("model", p.model.trim().to_owned())
        .part("file", part);
    if !language.trim().is_empty() {
        form = form.text("language", language.trim().to_owned());
    }
    let resp = authorize(http.post(endpoint(p, "audio/transcriptions")), p)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(p.timeout_secs.max(30)))
        .send()
        .await
        .context("Couldn't reach the transcription endpoint")?;
    if !resp.status().is_success() {
        return Err(error_from_response(resp).await);
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .context("Unexpected transcription response")?;
    let text = v
        .get("text")
        .and_then(|t| t.as_str())
        .context("The transcription response has no text")?;
    Ok(text.trim().to_owned())
}

/// `POST {base}/audio/speech`: the reply's text as WAV audio.
pub async fn synthesize(
    http: &reqwest::Client,
    p: &ProviderConfig,
    voice: &str,
    speed: f32,
    text: &str,
) -> Result<Vec<u8>> {
    let body = speak::tts_body(p.model.trim(), voice.trim(), speed, text);
    let resp = authorize(http.post(endpoint(p, "audio/speech")), p)
        .json(&body)
        .timeout(std::time::Duration::from_secs(p.timeout_secs.max(30)))
        .send()
        .await
        .context("Couldn't reach the speech endpoint")?;
    if !resp.status().is_success() {
        return Err(error_from_response(resp).await);
    }
    let bytes = resp.bytes().await?.to_vec();
    speak::parse_wav(&bytes).map_err(anyhow::Error::msg)?;
    Ok(bytes)
}

/// The newest reply with some text in it, as its message index.
pub fn latest_reply(messages: &[ChatMessage]) -> Option<usize> {
    messages
        .iter()
        .rposition(|m| m.role == Role::Assistant && !m.content.trim().is_empty())
}

/// What the voice features are doing right now.
#[derive(Default)]
pub struct VoiceState {
    pub recording: Option<Recording>,
    pub transcribing: bool,
    pub speaking: Option<Speaking>,
}

pub struct Recording {
    pub started: Instant,
    stop: Arc<AtomicBool>,
}

pub struct Speaking {
    pub key: SpeechKey,
    stop: Arc<AtomicBool>,
}

impl VoiceState {
    /// Starts a recording. The thread reports the clip as `AppEvent::VoiceCaptured`.
    pub fn start_recording(
        &mut self,
        max_secs: u32,
        tx: &EventSender,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), String> {
        let stop = Arc::new(AtomicBool::new(false));
        let tx = tx.clone();
        record::start(max_secs, stop.clone(), move |result| {
            let _ = tx.send(AppEvent::VoiceCaptured(result));
            repaint();
        })?;
        self.recording = Some(Recording {
            started: Instant::now(),
            stop,
        });
        Ok(())
    }

    /// Ends the recording; the clip arrives as `AppEvent::VoiceCaptured`.
    pub fn stop_recording(&mut self) {
        if let Some(r) = &self.recording {
            r.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Stops reading aloud, if anything is being read.
    pub fn stop_speaking(&mut self) {
        if let Some(s) = self.speaking.take() {
            s.stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn speaking_key(&self) -> Option<SpeechKey> {
        self.speaking.as_ref().map(|s| s.key)
    }

    /// Starts reading `text` aloud for `key`, replacing any reading in progress. `notify` gets the
    /// `SpeechEnded` event when the reading finishes, is stopped, or fails.
    pub fn start_speaking(
        &mut self,
        rt: &tokio::runtime::Handle,
        http: &reqwest::Client,
        settings: &Settings,
        key: SpeechKey,
        text: String,
        notify: impl Fn(AppEvent) + Send + Sync + 'static,
    ) {
        self.stop_speaking();
        let stop = Arc::new(AtomicBool::new(false));
        self.speaking = Some(Speaking {
            key,
            stop: stop.clone(),
        });
        let out = &settings.general.voice.output;
        let reporter = Reporter::new(
            "Reading aloud stopped unexpectedly",
            move |result: Result<(), String>| {
                notify(AppEvent::SpeechEnded { key, result });
            },
        );
        match out.engine {
            SpeechEngine::OsVoice => {
                let speed = out.speed;
                std::thread::Builder::new()
                    .name("voice-speak".into())
                    .spawn(move || {
                        let cmd = speak::os_voice_command(
                            std::env::consts::OS,
                            speed,
                            &text,
                            |p| which::which(p).is_ok(),
                        );
                        reporter.finish(match cmd {
                            Some(cmd) => speak::run_os_voice(&cmd, &stop),
                            None => Err(
                                "No speech program found. Install spd-say or espeak-ng to read replies aloud"
                                    .into(),
                            ),
                        });
                    })
                    .map_or_else(|e| log::warn!("speech thread: {e}"), |_| ());
            }
            SpeechEngine::Endpoint => {
                let (p, voice, speed, http) = (
                    output_provider(settings),
                    out.voice.clone(),
                    out.speed,
                    http.clone(),
                );
                rt.spawn(async move {
                    let result = async {
                        let wav = synthesize(&http, &p, &voice, speed, &text).await?;
                        if stop.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        tokio::task::spawn_blocking(move || speak::play_wav(&wav, &stop))
                            .await
                            .context("Playback stopped unexpectedly")?
                            .map_err(anyhow::Error::msg)
                    }
                    .await
                    .map_err(|e| format!("{e:#}"));
                    reporter.finish(result);
                });
            }
        }
    }
}

/// Transcribes a clip in the background; the text arrives as `AppEvent::VoiceTranscribed`.
pub fn spawn_transcription(
    rt: &tokio::runtime::Handle,
    http: &reqwest::Client,
    settings: &Settings,
    wav: Vec<u8>,
    tx: &EventSender,
    repaint: impl Fn() + Send + Sync + 'static,
) {
    let p = input_provider(settings);
    let language = settings.general.voice.input.language.clone();
    let (http, tx) = (http.clone(), tx.clone());
    rt.spawn(async move {
        let result = transcribe(&http, &p, &language, wav)
            .await
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(AppEvent::VoiceTranscribed(result));
        repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{self, Response};

    fn provider(base: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: format!("{base}/v1"),
            api_key: "sk-voice".into(),
            ..Default::default()
        }
    }

    #[test]
    fn reporter_reports_once_and_an_error_when_dropped_unreported() {
        let seen: Arc<std::sync::Mutex<Vec<Result<u8, String>>>> = Default::default();
        let s = seen.clone();
        Reporter::new("lost", move |r: Result<u8, String>| {
            s.lock().unwrap().push(r)
        })
        .finish(Ok(1));
        let s = seen.clone();
        drop(Reporter::new("lost", move |r: Result<u8, String>| {
            s.lock().unwrap().push(r)
        }));
        assert_eq!(*seen.lock().unwrap(), vec![Ok(1), Err("lost".to_owned())]);
    }

    #[test]
    fn latest_reply_skips_empty_and_user_messages() {
        let msg = |role: Role, text: &str| ChatMessage {
            content: text.into(),
            ..ChatMessage::new(role, text)
        };
        let msgs = vec![
            msg(Role::Assistant, "one"),
            msg(Role::User, "q"),
            msg(Role::Assistant, "  "),
        ];
        assert_eq!(latest_reply(&msgs), Some(0));
        assert_eq!(latest_reply(&msgs[1..2]), None);
    }

    #[test]
    fn settings_without_voice_still_load_with_defaults() {
        let s: VoiceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, VoiceSettings::default());
        assert!(!s.input.enabled && !s.output.enabled);
        assert_eq!(s.input.model, "whisper-1");
        assert_eq!(s.output.model, "tts-1");
        assert_eq!(s.output.voice, "alloy");
        assert_eq!(s.input.max_secs, 120);
        assert_eq!(s.output.speed, 1.0);
        assert_eq!(s.output.max_chars, 4000);
        assert_eq!(s.output.engine, SpeechEngine::OsVoice);
    }

    #[test]
    fn settings_files_from_before_voice_still_load() {
        let s: Settings = serde_json::from_str(r#"{"general":{"theme":"Dark"}}"#).unwrap();
        assert_eq!(s.general.voice, VoiceSettings::default());
        let mut voice = VoiceSettings::default();
        voice.output.engine = SpeechEngine::Endpoint;
        voice.input.provider = Some("p9".into());
        let mut s = Settings::default();
        s.general.voice = voice.clone();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.general.voice, voice);
        let json = serde_json::to_value(&voice).unwrap();
        assert_eq!(json["output"]["engine"], "endpoint");
    }

    #[test]
    fn partial_voice_settings_keep_what_they_name() {
        let s: VoiceSettings =
            serde_json::from_str(r#"{"output":{"enabled":true,"engine":"endpoint","speed":1.5}}"#)
                .unwrap();
        assert!(s.output.enabled);
        assert_eq!(s.output.engine, SpeechEngine::Endpoint);
        assert_eq!(s.output.speed, 1.5);
        assert_eq!(s.output.voice, "alloy");
    }

    #[test]
    fn normalize_clamps_and_drops_unknown_providers() {
        let mut s = VoiceSettings::default();
        s.input.provider = Some("gone".into());
        s.input.model = "  ".into();
        s.input.max_secs = 9_999;
        s.output.speed = f32::NAN;
        s.output.max_chars = 5;
        s.output.voice = String::new();
        let providers = vec![ProviderConfig {
            id: "p1".into(),
            ..Default::default()
        }];
        s.normalize(&providers);
        assert_eq!(s.input.provider, None);
        assert_eq!(s.input.model, "whisper-1");
        assert_eq!(s.input.max_secs, MAX_RECORD_SECS);
        assert_eq!(s.output.speed, 1.0);
        assert_eq!(s.output.max_chars, 200);
        assert_eq!(s.output.voice, "alloy");

        s.output.provider = Some("p1".into());
        s.output.speed = 9.0;
        s.normalize(&providers);
        assert_eq!(s.output.provider.as_deref(), Some("p1"));
        assert_eq!(s.output.speed, 2.0);
    }

    #[test]
    fn voice_calls_use_the_chosen_profile_or_the_active_one() {
        let mut settings = Settings::default();
        let mut other = ProviderConfig {
            id: "groq".into(),
            base_url: "https://api.groq.com/openai/v1".into(),
            ..Default::default()
        };
        other.model = "chat-model".into();
        settings.providers.push(other);
        settings.general.voice.input.provider = Some("groq".into());
        settings.general.voice.input.model = "whisper-large".into();
        let p = input_provider(&settings);
        assert_eq!(p.base_url, "https://api.groq.com/openai/v1");
        assert_eq!(p.model, "whisper-large");

        settings.general.voice.input.provider = None;
        let p = input_provider(&settings);
        assert_eq!(p.id, settings.provider().id);
        assert_eq!(p.model, "whisper-large");
        assert_eq!(output_provider(&settings).model, "tts-1");
    }

    #[test]
    fn transcript_goes_at_the_cursor_or_the_end_with_spacing() {
        let mut s = String::from("Hello");
        insert_text(&mut s, None, "  world  ");
        assert_eq!(s, "Hello world");
        insert_text(&mut s, None, "   ");
        assert_eq!(s, "Hello world");

        let mut s = String::from("ab cd");
        insert_text(&mut s, Some(2), "X");
        assert_eq!(s, "ab X cd");
        let mut s = String::from("ab cd");
        insert_text(&mut s, Some(0), "X");
        assert_eq!(s, "X ab cd");
        let mut s = String::from("é!");
        insert_text(&mut s, Some(1), "ok");
        assert_eq!(s, "é ok!", "cursor counts characters, not bytes");
        let mut s = String::from("ab");
        insert_text(&mut s, Some(1), "X");
        assert_eq!(s, "a X b");
        let mut s = String::new();
        insert_text(&mut s, Some(10), "start");
        assert_eq!(s, "start");
    }

    #[tokio::test]
    async fn transcription_posts_the_clip_as_multipart_with_the_key() {
        let (base, log) =
            testutil::serve(|_| Response::json(serde_json::json!({"text": " hi there "}))).await;
        let mut p = provider(&base);
        p.model = "whisper-1".into();
        let text = transcribe(&reqwest::Client::new(), &p, "de", vec![1, 2, 3])
            .await
            .unwrap();
        assert_eq!(text, "hi there");
        let seen = log.lock().unwrap();
        let req = &seen[0];
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/v1/audio/transcriptions");
        assert_eq!(req.header("authorization"), Some("Bearer sk-voice"));
        let ct = req.header("content-type").unwrap();
        assert!(ct.starts_with("multipart/form-data"), "{ct}");
        let body = &req.body;
        let has = |needle: &[u8]| body.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"name=\"model\"\r\n\r\nwhisper-1"));
        assert!(has(b"name=\"language\"\r\n\r\nde"));
        assert!(has(b"name=\"file\"; filename=\"speech.wav\""));
        assert!(has(&[1u8, 2, 3]), "the clip itself is in the body");
    }

    #[tokio::test]
    async fn transcription_reports_endpoint_errors() {
        let (base, _) = testutil::serve(|_| {
            Response::with_type(
                401,
                "application/json",
                r#"{"error":{"message":"bad key"}}"#,
            )
        })
        .await;
        let err = transcribe(&reqwest::Client::new(), &provider(&base), "", vec![])
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("bad key"), "{err:#}");
    }

    #[tokio::test]
    async fn speech_request_asks_for_wav_and_returns_it() {
        let wav = record::encode_wav(&[0, 1000, -1000], 16_000);
        let reply = wav.clone();
        let (base, log) =
            testutil::serve(move |_| Response::with_type(200, "audio/wav", reply.clone())).await;
        let mut p = provider(&base);
        p.model = "tts-1-hd".into();
        let got = synthesize(&reqwest::Client::new(), &p, "nova", 1.5, "Read me")
            .await
            .unwrap();
        assert_eq!(got, wav);
        let seen = log.lock().unwrap();
        assert_eq!(seen[0].path, "/v1/audio/speech");
        let body = seen[0].json();
        assert_eq!(body["model"], "tts-1-hd");
        assert_eq!(body["voice"], "nova");
        assert_eq!(body["input"], "Read me");
        assert_eq!(body["response_format"], "wav");
        assert_eq!(body["speed"], 1.5);
    }

    #[tokio::test]
    async fn speech_endpoint_that_sends_something_else_is_an_error() {
        let (base, _) =
            testutil::serve(|_| Response::with_type(200, "audio/mpeg", b"ID3 not wav".to_vec()))
                .await;
        let err = synthesize(&reqwest::Client::new(), &provider(&base), "alloy", 1.0, "x")
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("WAV"), "{err:#}");
    }
}
