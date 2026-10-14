//! Translation via an OpenAI-compatible chat endpoint.
//!
//! OpenWhispr offers a "dictate in one language, paste in another" hotkey. This
//! is the same idea: after transcription, send the text to a chat model and ask
//! for a translation.
//!
//! Works against OpenAI (`/chat/completions`) or any compatible server — point
//! `base_url` at a local llama.cpp / Ollama / LM Studio endpoint and the audio
//! never leaves the machine. Note: the bundled Whisper server does NOT serve
//! `/chat/completions`, so translation needs a separate chat endpoint.

use anyhow::{anyhow, bail, Result};

pub struct TranslateConfig {
    pub api_key: String,
    /// Chat model name, e.g. `gpt-4o-mini` or a local model name.
    pub model: String,
    pub base_url: String,
    /// Target language name or code, e.g. "Indonesian" or "id".
    pub target: String,
}

/// Translate `text` into `cfg.target`. Returns the translated string.
pub async fn translate(text: &str, cfg: &TranslateConfig) -> Result<String> {
    if text.trim().is_empty() {
        bail!("nothing to translate");
    }
    if cfg.target.trim().is_empty() {
        bail!("no target language configured");
    }

    let url = format!(
        "{}/chat/completions",
        cfg.base_url.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "model": cfg.model,
        "temperature": 0,
        "messages": [
            {
                "role": "system",
                "content": format!(
                    "You are a translation engine. Translate the user's text into {}. \
                     Output ONLY the translation, with no quotes, notes, or explanations. \
                     Preserve the original tone and formatting.",
                    cfg.target
                )
            },
            { "role": "user", "content": text }
        ]
    });

    let mut req = reqwest::Client::new().post(&url).json(&body);
    if !cfg.api_key.trim().is_empty() {
        req = req.bearer_auth(cfg.api_key.trim());
    }

    let resp = req.send().await.map_err(|e| anyhow!("request failed: {e}"))?;
    let status = resp.status();
    let raw = resp.text().await.map_err(|e| anyhow!("could not read response: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| anyhow!("bad response ({status}): {e}: {raw}"))?;

    if !status.is_success() {
        let msg = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        bail!("translation failed ({status}): {msg}");
    }

    let out = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if out.is_empty() {
        bail!("the model returned no translation");
    }
    Ok(out)
}
