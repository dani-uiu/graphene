//! Approximate token counts for `graphene tokens` (D-019).
//!
//! Depends on: `tiktoken-rs` when the `tokens` feature is enabled;
//! nothing otherwise.
//!
//! Invariants: counts are approximate and labeled with their method. They
//! are not Claude tokenizer counts.

/// Name of the counting method in use.
pub fn method() -> &'static str {
    if cfg!(feature = "tokens") {
        "tiktoken o200k_base (approximate)"
    } else {
        "heuristic: characters / 4 (build with --features tokens for tiktoken)"
    }
}

#[cfg(feature = "tokens")]
pub fn count(text: &str) -> usize {
    match tiktoken_rs::o200k_base() {
        Ok(bpe) => bpe.encode_with_special_tokens(text).len(),
        Err(_) => heuristic(text),
    }
}

#[cfg(not(feature = "tokens"))]
pub fn count(text: &str) -> usize {
    heuristic(text)
}

fn heuristic(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}
