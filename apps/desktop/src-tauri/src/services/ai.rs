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
            Ok(_) => true,
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
        serde_json::json!({
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
        })
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
}
