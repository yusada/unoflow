//! Transcription, extracted so UnoFlow can target either the OpenAI
//! audio-transcriptions API or any OpenAI-compatible server (e.g. a local
//! `whisper.cpp` `whisper-server`).
//!
//! The public surface is intentionally tiny:
//!   * [`TranscribeConfig`] — where to send the request and how to authenticate
//!   * [`pcm16_to_wav`]     — wrap raw PCM in a WAV container
//!   * [`transcribe_pcm`]   — POST that WAV and return the transcript

use anyhow::{anyhow, bail, Result};

/// Where and how to send a transcription request.
pub struct TranscribeConfig {
    /// Bearer token. May be empty when `base_url` points at a server that
    /// needs no authentication (e.g. a local `whisper.cpp` server).
    pub api_key: String,
    /// Model name, e.g. `gpt-4o-mini-transcribe` or `whisper-1`.
    pub model: String,
    /// Optional language hint (e.g. `"en"`). Omitted from the request when empty.
    pub language: String,
    /// Optional context: text from the focused control, so the model gets
    /// names/jargon right. Omitted from the request when empty.
    pub prompt: String,
    /// Base URL of the API, e.g. `"https://api.openai.com/v1"` (default) or
    /// `"http://127.0.0.1:8080/v1"`.
    pub base_url: String,
}

/// Host we treat as "the OpenAI API": an empty key is only tolerated when the
/// request is *not* headed there.
const OPENAI_HOST: &str = "api.openai.com";

/// Wrap raw 16-bit little-endian mono PCM in a minimal WAV container.
pub fn pcm16_to_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let bits = 16u16;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * block_align as u32;
    let data_len = pcm.len() as u32;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// Whether `base_url` points at the OpenAI API (and therefore requires a key).
fn is_openai_host(base_url: &str) -> bool {
    base_url.to_ascii_lowercase().contains(OPENAI_HOST)
}

/// Join a base URL with a path, tolerating a trailing slash on the base.
fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}/{}", base_url.trim_end_matches('/'), path)
}

/// POST the PCM (as WAV) to `{base_url}/audio/transcriptions` as multipart and
/// return the trimmed `text`.
pub async fn transcribe_pcm(
    pcm: &[u8],
    sample_rate: u32,
    cfg: &TranscribeConfig,
) -> Result<String> {
    if pcm.is_empty() {
        bail!("no audio captured");
    }

    let needs_key = is_openai_host(&cfg.base_url);
    if cfg.api_key.trim().is_empty() && needs_key {
        bail!(
            "no API key set: add an OpenAI key in settings (or set \
             UNOFLOW_OPENAI_KEY). To use a local server instead, point the \
             base URL at it (e.g. http://127.0.0.1:8080/v1)."
        );
    }

    let wav = pcm16_to_wav(pcm, sample_rate, 1);
    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| anyhow!("bad mime type: {e}"))?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", cfg.model.clone());
    if !cfg.language.is_empty() {
        form = form.text("language", cfg.language.clone());
    }
    if !cfg.prompt.is_empty() {
        form = form.text("prompt", cfg.prompt.clone());
    }

    let url = endpoint(&cfg.base_url, "audio/transcriptions");
    let mut req = reqwest::Client::new().post(&url).multipart(form);
    // Local/self-hosted servers often accept no key; only send the header when
    // we actually have one.
    if !cfg.api_key.trim().is_empty() {
        req = req.bearer_auth(cfg.api_key.trim());
    }

    let resp = req.send().await.map_err(|e| anyhow!("request failed: {e}"))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| anyhow!("could not read response: {e}"))?;
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow!("bad response ({status}): {e}: {body}"))?;

    if !status.is_success() {
        let msg = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        bail!("transcription failed ({status}): {msg}");
    }

    Ok(json
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .trim()
        .to_string())
}
