//! AIProvider abstraction (spec §34, §47.7).
//!
//! Every AI feature goes through the [`AiProvider`] trait so the
//! llama.cpp implementation can be swapped (cloud add-on, future runtimes)
//! without touching call sites. Two implementations ship in Phase 3:
//!
//! - [`EchoProvider`] — deterministic, offline, no dependencies. Used in
//!   tests and as the visible "model not loaded" fallback; its output is
//!   clearly labelled so it can never be mistaken for a real answer.
//! - [`LlamaCppProvider`] — talks to a locally running `llama-server`
//!   (llama.cpp) on loopback via its OpenAI-compatible API. The core never
//!   links llama.cpp itself; the runtime is a separate process (crash
//!   isolation, instant "unload" = kill, spec §43).
//!
//! Grounding rules (spec §16/§31) live in `analysis.rs`; this module is
//! transport only.

use std::io::BufRead;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::{AppError, AppResult};

/// One ask request. Multi-turn history is flattened by the caller
/// (`analysis.rs`) into the user prompt so providers stay single-shot.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub system_prompt: String,
    pub user_prompt: String,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Ask the chat template to skip chain-of-thought (`enable_thinking`).
    /// Reasoning tokens are billed against `max_tokens` but never reach
    /// `content`, so a narration script can come back empty with thinking on.
    pub disable_thinking: bool,
}

// ---------------------------------------------------------------------------
// Prompt sizing — llama-server rejects an over-long prompt outright (HTTP 400,
// `exceed_context_size_error`), so prompts must be budgeted in *tokens* before
// they are sent. Without a tokenizer we estimate per character: Latin-ish text
// runs ~3.6 chars/token, CJK/Hangul/Kana/emoji ~1 token each. Estimating high
// only clips an excerpt a little earlier; estimating low fails the request.
// ---------------------------------------------------------------------------

/// Rough token cost of one character.
fn tokens_per_char(c: char) -> f64 {
    // CJK ideograms, radicals, kana, Hangul, fullwidth forms, emoji.
    let wide = matches!(
        c as u32,
        0x1100..=0x11FF
            | 0x2E80..=0x303F
            | 0x3040..=0x30FF
            | 0x3100..=0x312F
            | 0x3130..=0x318F
            | 0x31C0..=0x31EF
            | 0x3200..=0x32FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFFEF
            | 0x1F000..=0x1FAFF
            | 0x20000..=0x2FFFF
    );
    if wide { 1.1 } else { 0.28 }
}

/// Estimated token count of `s` (chat-template overhead not included).
pub fn est_tokens(s: &str) -> usize {
    let mut cost = 0.0f64;
    for c in s.chars() {
        cost += tokens_per_char(c);
    }
    cost.ceil() as usize
}

/// Clip `text` to a rough token budget. The cut lands on a character
/// boundary (byte slicing panics on multi-byte text) and prefers a sentence
/// ending, so the excerpt reads as complete sentences.
pub fn clip_to_tokens(text: &str, budget: usize) -> String {
    let mut cost = 0.0f64;
    let mut byte_end = text.len();
    for (i, c) in text.char_indices() {
        cost += tokens_per_char(c);
        // Compare the *rounded* cost: est_tokens() of the cut must never
        // exceed the budget by even one token, or the request can 400.
        if cost.ceil() as usize > budget {
            byte_end = i;
            break;
        }
    }
    if byte_end == text.len() {
        return text.to_string();
    }
    let cut = &text[..byte_end];
    match cut.rfind(". ") {
        Some(i) => cut[..i + 2].to_string(), // keep the ". " so the clip ends on a sentence
        None => cut.to_string(),
    }
}

/// Result of one completion. `engine`/`model` land in the UI debug panel.
#[derive(Debug, Clone)]
pub struct CompletionOutput {
    pub text: String,
    /// "llama.cpp" | "echo" | …
    pub engine: String,
    pub model: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub duration_ms: u64,
    /// "stop" | "length" | …
    pub finish_reason: Option<String>,
}

pub trait AiProvider: Send + Sync {
    fn name(&self) -> &'static str;

    /// Blocking completion (whole answer at once).
    fn complete(&self, req: &CompletionRequest) -> AppResult<CompletionOutput>;

    /// Streaming completion; `on_delta` receives incremental text chunks.
    /// Returns the full output when the stream finishes.
    fn complete_stream(
        &self,
        req: &CompletionRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> AppResult<CompletionOutput>;
}

// ---------------------------------------------------------------------------
// EchoProvider — deterministic offline fallback (tests + "model not loaded")
// ---------------------------------------------------------------------------

pub struct EchoProvider;

impl AiProvider for EchoProvider {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn complete(&self, req: &CompletionRequest) -> AppResult<CompletionOutput> {
        self.complete_stream(req, &mut |_| {})
    }

    fn complete_stream(
        &self,
        req: &CompletionRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> AppResult<CompletionOutput> {
        let started = Instant::now();
        let text = format!(
            "[Echo provider — no local model is loaded, this is not a real answer.]\n\n\
             System prompt received: {}\n\n\
             Your request was:\n{}",
            req.system_prompt.trim(),
            req.user_prompt.trim(),
        );

        // Emit in word-chunks so the streaming path of callers is exercised
        // the same way a real token stream would be. The returned `text` is
        // always the exact concatenation of the deltas (stream invariant).
        let mut acc = String::new();
        for chunk in text.split_whitespace().collect::<Vec<_>>().chunks(6) {
            let s = format!("{}\n", chunk.join(" "));
            acc.push_str(&s);
            on_delta(&s);
        }

        Ok(CompletionOutput {
            text: acc,
            engine: "echo".into(),
            model: None,
            prompt_tokens: None,
            completion_tokens: None,
            duration_ms: started.elapsed().as_millis() as u64,
            finish_reason: Some("stop".into()),
        })
    }
}

// ---------------------------------------------------------------------------
// LlamaCppProvider — llama-server's OpenAI-compatible API on loopback
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LlamaCppProvider {
    agent: ureq::Agent,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl LlamaCppProvider {
    pub fn new(base_url: &str, model: &str, api_key: Option<String>) -> Self {
        let config = ureq::Agent::config_builder()
            // Long generations on CPU-bound 8 GB machines; the global timeout
            // caps a full request+body read at 10 minutes.
            .timeout_global(Some(Duration::from_secs(600)))
            // Return non-2xx responses instead of turning them into errors,
            // so `status_error` can show llama-server's explanation ("request
            // (5699 tokens) exceeds the available context size") rather than
            // the bare "http status: 400".
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// True when llama-server answers /health with an OK status (model
    /// loaded and ready; 503 while it is still loading weights).
    pub fn health_ok(&self) -> bool {
        match self.agent.get(format!("{}/health", self.base_url)).call() {
            // With http_status_as_error off, 503 (still loading) arrives as a
            // normal response — only 2xx means ready.
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    fn post_chat(&self, body: serde_json::Value) -> AppResult<ureq::http::Response<ureq::Body>> {
        let mut req = self
            .agent
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Content-Type", "application/json");
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        req.send_json(body)
            .map_err(|e| AppError::msg(format!("Local model request failed: {e}")))
    }

    fn request_body(&self, req: &CompletionRequest, stream: bool) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": req.system_prompt },
                { "role": "user", "content": req.user_prompt },
            ],
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "stream": stream,
            // llama-server honours this OpenAI option and sends a final
            // usage-only chunk — lets us report token counts on streams too.
            "stream_options": { "include_usage": true },
        });
        if req.disable_thinking {
            // Qwen3-class chat templates think by default; reasoning tokens
            // spend `max_tokens` without ever reaching `content`. The bundled
            // llama-server renders templates with Jinja on, and this kwarg
            // turns thinking off for this one request (unused kwargs are
            // ignored by templates that don't reference them).
            body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": false });
        }
        body
    }

    fn status_error(mut resp: ureq::http::Response<ureq::Body>) -> AppError {
        let status = resp.status();
        let detail = resp
            .body_mut()
            .read_to_string()
            .unwrap_or_default();
        let detail = detail.trim();
        AppError::msg(if detail.is_empty() {
            format!("Local model returned HTTP {status}.")
        } else {
            format!("Local model returned HTTP {status}: {detail}")
        })
    }
}

impl AiProvider for LlamaCppProvider {
    fn name(&self) -> &'static str {
        "llama.cpp"
    }

    fn complete(&self, req: &CompletionRequest) -> AppResult<CompletionOutput> {
        let started = Instant::now();
        let mut resp = self.post_chat(self.request_body(req, false))?;
        if !resp.status().is_success() {
            return Err(Self::status_error(resp));
        }
        let body: LlamaChatResponse = resp
            .body_mut()
            .read_json()
            .map_err(|e| AppError::msg(format!("Invalid response from local model: {e}")))?;

        let choice = body.choices.into_iter().next().ok_or_else(|| {
            AppError::msg("Local model response contained no choices.")
        })?;
        Ok(CompletionOutput {
            text: choice.message.content,
            engine: "llama.cpp".into(),
            model: body.model.or_else(|| Some(self.model.clone())),
            prompt_tokens: body.usage.as_ref().map(|u| u.prompt_tokens),
            completion_tokens: body.usage.as_ref().map(|u| u.completion_tokens),
            duration_ms: started.elapsed().as_millis() as u64,
            finish_reason: choice.finish_reason,
        })
    }

    fn complete_stream(
        &self,
        req: &CompletionRequest,
        on_delta: &mut dyn FnMut(&str),
    ) -> AppResult<CompletionOutput> {
        let started = Instant::now();
        let resp = self.post_chat(self.request_body(req, true))?;
        if !resp.status().is_success() {
            return Err(Self::status_error(resp));
        }

        let reader = resp.into_body().into_reader();
        let mut text = String::new();
        let mut prompt_tokens = None;
        let mut completion_tokens = None;
        let mut finish_reason = None;
        let mut model = self.model.clone();

        for line in std::io::BufReader::new(reader).lines() {
            let line = line.map_err(|e| AppError::msg(format!("Model stream broke: {e}")))?;
            let payload = match line.strip_prefix("data:") {
                Some(p) => p.trim(),
                None => continue, // SSE comments / "event:" lines
            };
            if payload == "[DONE]" {
                break;
            }
            let Ok(chunk) = serde_json::from_str::<LlamaStreamChunk>(payload) else {
                continue; // keep-alive noise etc. — never fail the stream on junk
            };
            if let Some(u) = chunk.usage {
                prompt_tokens = Some(u.prompt_tokens);
                completion_tokens = Some(u.completion_tokens);
            }
            if let Some(m) = chunk.model {
                model = m;
            }
            if let Some(choice) = chunk.choices.into_iter().next() {
                if let Some(fr) = choice.finish_reason {
                    finish_reason = Some(fr);
                }
                if let Some(d) = choice.delta.and_then(|d| d.content) {
                    if !d.is_empty() {
                        on_delta(&d);
                        text.push_str(&d);
                    }
                }
            }
        }

        Ok(CompletionOutput {
            text,
            engine: "llama.cpp".into(),
            model: Some(model),
            prompt_tokens,
            completion_tokens,
            duration_ms: started.elapsed().as_millis() as u64,
            finish_reason,
        })
    }
}

// -- wire types (OpenAI-compatible subset actually used) --------------------

#[derive(Debug, Deserialize)]
struct LlamaChatResponse {
    #[serde(default)]
    model: Option<String>,
    choices: Vec<LlamaChoice>,
    #[serde(default)]
    usage: Option<LlamaUsage>,
}

#[derive(Debug, Deserialize)]
struct LlamaChoice {
    message: LlamaMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlamaMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct LlamaStreamChunk {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<LlamaDeltaChoice>,
    #[serde(default)]
    usage: Option<LlamaUsage>,
}

#[derive(Debug, Deserialize)]
struct LlamaDeltaChoice {
    #[serde(default)]
    delta: Option<LlamaDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlamaDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlamaUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

// ---------------------------------------------------------------------------
// Tests — the SSE/JSON parsing is exercised against a canned loopback HTTP
// server, so the full ureq path runs without llama.cpp installed.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One-shot canned HTTP server: accepts a single connection, reads the
    /// whole request (Content-Length aware), writes the full response, and
    /// closes the socket gracefully. The graceful half-close and drain
    /// matter on Windows, where a hard close on an unread socket can abort
    /// the client's pending read with os error 10053 before it sees the
    /// response bytes.
    fn serve_once(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            // Read until end of headers, then up to Content-Length more bytes.
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match sock.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        raw.extend_from_slice(&buf[..n]);
                        let head_end = raw
                            .windows(4)
                            .position(|w| w == b"\r\n\r\n")
                            .map(|p| p + 4);
                        if let Some(head_end) = head_end {
                            let len = String::from_utf8_lossy(&raw[..head_end])
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse::<usize>().ok())?
                                })
                                .unwrap_or(0);
                            if raw.len() >= head_end + len {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = sock.write_all(body.as_bytes());
            let _ = sock.flush();
            let _ = sock.shutdown(std::net::Shutdown::Write);
            // Drain until the peer closes so no response bytes are lost to
            // a platform RST-on-close race.
            while let Ok(n) = sock.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        });
        format!("http://{addr}")
    }

    /// Build a complete HTTP/1.1 response with correct Content-Length framing.
    fn http_response(content_type: &str, body: &str) -> &'static str {
        Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        )
    }

    fn sample_request() -> CompletionRequest {
        CompletionRequest {
            system_prompt: "Answer only from the numbered evidence.".into(),
            user_prompt: "What does [1] claim?".into(),
            max_tokens: 256,
            temperature: 0.2,
            disable_thinking: false,
        }
    }

    #[test]
    fn echo_provider_is_deterministic_and_labelled() {
        let p = EchoProvider;
        let req = sample_request();
        let a = p.complete(&req).unwrap();
        let b = p.complete(&req).unwrap();
        assert_eq!(a.text, b.text);
        assert_eq!(a.engine, "echo");
        assert!(a.text.contains("Echo provider"));
        assert!(a.text.contains("What does [1] claim?"));
        assert!(a.finish_reason.as_deref() == Some("stop"));
    }

    #[test]
    fn echo_stream_matches_complete_text() {
        let p = EchoProvider;
        let req = sample_request();
        let mut streamed = String::new();
        let out = p
            .complete_stream(&req, &mut |d| streamed.push_str(d))
            .unwrap();
        assert_eq!(streamed, out.text);
        assert!(!streamed.is_empty());
    }

    #[test]
    fn llama_provider_parses_sse_stream_with_usage() {
        let sse = concat!(
            "data: {\"model\":\"qwen-test\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
        );
        let url = serve_once(http_response("text/event-stream", sse));
        let p = LlamaCppProvider::new(&url, "qwen-test", None);

        let mut chunks = Vec::new();
        let out = p
            .complete_stream(&sample_request(), &mut |d| chunks.push(d.to_string()))
            .unwrap();

        assert_eq!(out.text, "Hello");
        assert_eq!(chunks.join(""), "Hello");
        assert_eq!(out.engine, "llama.cpp");
        assert_eq!(out.model.as_deref(), Some("qwen-test"));
        assert_eq!(out.prompt_tokens, Some(5));
        assert_eq!(out.completion_tokens, Some(2));
    }

    #[test]
    fn llama_provider_parses_non_stream_response() {
        let body = r#"{"model":"qwen-test","choices":[{"message":{"role":"assistant","content":"Cited answer."},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":11,"completion_tokens":3}}"#;
        let url = serve_once(http_response("application/json", body));
        let p = LlamaCppProvider::new(&url, "qwen-test", None);

        let out = p.complete(&sample_request()).unwrap();
        assert_eq!(out.text, "Cited answer.");
        assert_eq!(out.finish_reason.as_deref(), Some("stop"));
        assert_eq!(out.prompt_tokens, Some(11));
        assert_eq!(out.completion_tokens, Some(3));
    }

    #[test]
    fn llama_provider_surfaces_http_errors_humanely() {
        let url = serve_once(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: 47\r\nConnection: close\r\n\r\n{\"error\":{\"message\":\"loading model weights\"}}\n\n",
        );
        let p = LlamaCppProvider::new(&url, "qwen-test", None);
        let err = p.complete(&sample_request()).unwrap_err().to_string();
        assert!(err.contains("503"), "error should carry status: {err}");
    }

    /// Regression: an over-long prompt used to surface as the useless toast
    /// "Local model request failed: http status: 400" because ureq swallowed
    /// the body. The server's explanation is the whole diagnosis.
    #[test]
    fn llama_provider_shows_the_server_body_on_a_400() {
        let body = "{\"error\":{\"code\":400,\"message\":\"request (5699 tokens) exceeds the available context size (4096 tokens), try increasing it\",\"type\":\"exceed_context_size_error\"}}";
        let resp = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let url = serve_once(Box::leak(resp.into_boxed_str()));
        let p = LlamaCppProvider::new(&url, "qwen-test", None);
        let err = p.complete(&sample_request()).unwrap_err().to_string();
        assert!(err.contains("400"), "{err}");
        assert!(err.contains("exceeds the available context size"), "{err}");
    }

    /// 503 means "still loading weights": health must read it as not-ready
    /// now that non-2xx responses are no longer turned into transport errors.
    #[test]
    fn health_ok_reads_503_as_not_ready() {
        let down = serve_once(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        );
        let p = LlamaCppProvider::new(&down, "qwen-test", None);
        assert!(!p.health_ok(), "503 must not read as ready");

        let up = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}");
        let p = LlamaCppProvider::new(&up, "qwen-test", None);
        assert!(p.health_ok(), "200 must read as ready");
    }

    #[test]
    fn est_tokens_is_conservative_for_both_scripts() {
        // English: ~4.2 chars/token in practice, we assume 3.6 → never under.
        let english = "Rural poverty programs combine cash transfers with training. ".repeat(50);
        let tokens = est_tokens(&english);
        assert!(english.len() / 5 < tokens && tokens < english.len() / 3, "{tokens}");

        // CJK is ~1.5 chars/token — a char-count budget would have sent 40% of
        // the real size and llama-server would 400.
        let cjk = "农村贫困问题需要综合性的政策干预。".repeat(50);
        assert_eq!(est_tokens(&cjk), (cjk.chars().count() as f64 * 1.1).ceil() as usize);

        assert_eq!(est_tokens(""), 0);
    }

    #[test]
    fn clip_to_tokens_respects_the_budget_and_multibyte_text() {
        let text = "One sentence here. ".repeat(500);
        let clipped = clip_to_tokens(&text, 100);
        assert!(est_tokens(&clipped) <= 100, "{} tokens", est_tokens(&clipped));
        assert!(clipped.ends_with(". "));

        // Cuts land on character boundaries: byte slicing would panic on any
        // of these (accents, emoji, CJK).
        let text = format!("{}{}", "x".repeat(5), "é".repeat(200));
        clip_to_tokens(&text, 7);
        let text = "👍".repeat(40);
        let clipped = clip_to_tokens(&text, 10);
        assert!(clipped.chars().count() <= 10);
        let text = "研究问题と方法。".repeat(30);
        assert!(text.starts_with(&clip_to_tokens(&text, 17)));

        assert_eq!(clip_to_tokens("whole text", 0), "");
        assert_eq!(clip_to_tokens("whole text", 1000), "whole text");
    }

    #[test]
    fn disable_thinking_adds_the_chat_template_kwarg_only_when_asked() {
        let p = LlamaCppProvider::new("http://127.0.0.1:1", "qwen-test", None);
        let mut req = sample_request();
        assert!(p.request_body(&req, false).get("chat_template_kwargs").is_none());
        req.disable_thinking = true;
        let body = p.request_body(&req, false);
        assert_eq!(
            body["chat_template_kwargs"]["enable_thinking"],
            serde_json::json!(false)
        );
    }
}
