//! Central error type. Human-readable messages only — raw stack traces are
//! never sent to the UI (spec §42).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Unsupported file type: {0}")]
    UnsupportedFileType(String),

    #[error("{0}")]
    Message(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn msg(message: impl Into<String>) -> Self {
        AppError::Message(message.into())
    }
}

/// Clip `s` to at most `max` bytes without ever splitting a UTF-8 character.
///
/// Used when truncating subprocess stderr for error messages: a raw
/// `&s[..max]` panics when `max` lands inside a multi-byte character —
/// lossy decoding inserts U+FFFD (3 bytes) and paths/warnings routinely
/// carry non-ASCII text, so a panic here would replace a helpful error with
/// a crash.
pub fn clip_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod clip_tests {
    use super::clip_bytes;

    #[test]
    fn short_and_exact_strings_pass_through() {
        assert_eq!(clip_bytes("hello", 10), "hello");
        assert_eq!(clip_bytes("hello", 5), "hello");
    }

    #[test]
    fn clips_ascii_at_the_limit() {
        assert_eq!(clip_bytes("abcdefgh", 4), "abcd");
    }

    #[test]
    fn never_splits_a_multibyte_character() {
        // Byte 1 falls inside "é" (2 bytes), byte 2 inside "€" (3 bytes).
        assert_eq!(clip_bytes("é€x", 1), "");
        assert_eq!(clip_bytes("é€x", 2), "é");
        assert_eq!(clip_bytes("é€x", 4), "é"); // 4 lands inside "€" (bytes 2–4)
        assert_eq!(clip_bytes("é€x", 5), "é€");
        assert_eq!(clip_bytes("é€x", 6), "é€x");
    }

    #[test]
    fn lossy_replacement_chars_truncate_cleanly() {
        // from_utf8_lossy turns invalid bytes into U+FFFD (3 bytes each).
        let s = String::from_utf8_lossy(b"ok \xff\xff\xff tail");
        for max in 0..=s.len() {
            let clipped = clip_bytes(&s, max);
            assert!(clipped.len() <= max);
            assert!(s.starts_with(clipped));
        }
    }
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
