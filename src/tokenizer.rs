// Accurate token counting for ctx% and compaction (ADR 0023), via tiktoken.
use crate::message::{Message, MessageItem};
use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

/// ~4 tokens of chat framing per message (role markers, delimiters).
const PER_MESSAGE_OVERHEAD: u64 = 4;

fn o200k() -> &'static CoreBPE {
    static B: OnceLock<CoreBPE> = OnceLock::new();
    B.get_or_init(|| tiktoken_rs::o200k_base().expect("o200k_base"))
}
fn cl100k() -> &'static CoreBPE {
    static B: OnceLock<CoreBPE> = OnceLock::new();
    B.get_or_init(|| tiktoken_rs::cl100k_base().expect("cl100k_base"))
}

/// The BPE encoding a model uses. gpt-4o / o-series / gpt-5 use o200k; older use cl100k.
fn bpe_for(model: &str) -> &'static CoreBPE {
    if model.starts_with("gpt-4o")
        || model.starts_with("gpt-5")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
    {
        o200k()
    } else {
        cl100k()
    }
}

fn count_text(bpe: &CoreBPE, text: &str) -> u64 {
    bpe.encode_ordinary(text).len() as u64
}

/// Total tokens of a message set for `model`, including per-message framing.
pub fn count_tokens(model: &str, messages: &[Message]) -> u64 {
    let bpe = bpe_for(model);
    let mut total = 0u64;
    for m in messages {
        total += PER_MESSAGE_OVERHEAD;
        for it in &m.items {
            total += match it {
                MessageItem::Text { text } | MessageItem::Reasoning { text } => count_text(bpe, text),
                MessageItem::ToolCall { name, args, .. } => {
                    count_text(bpe, name) + count_text(bpe, &args.to_string())
                }
                MessageItem::ToolResult { output, .. } => count_text(bpe, output),
            };
        }
    }
    total
}

/// The context-window size (tokens) for a model, for ctx% and compaction.
pub fn model_window(model: &str) -> u64 {
    let m = model;
    if m.starts_with("gpt-4o") || m.starts_with("gpt-4-turbo") || m.starts_with("gpt-4-1106")
        || m.starts_with("gpt-4-0125") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
    {
        128_000
    } else if m.starts_with("gpt-5") {
        256_000
    } else if m.starts_with("gpt-4-32k") {
        32_768
    } else if m.starts_with("gpt-4") {
        8_192
    } else if m.contains("16k") || m.starts_with("gpt-3.5") {
        16_385
    } else {
        128_000 // reasonable default for unknown/compatible endpoints
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;

    #[test]
    fn counts_are_positive_and_scale() {
        let short = vec![Message::text(0, Role::User, "hi")];
        let long = vec![Message::text(0, Role::User, "hello world ".repeat(50).trim())];
        let a = count_tokens("gpt-4o", &short);
        let b = count_tokens("gpt-4o", &long);
        assert!(a >= PER_MESSAGE_OVERHEAD);
        assert!(b > a);
    }

    #[test]
    fn windows_by_model() {
        assert_eq!(model_window("gpt-4o"), 128_000);
        assert_eq!(model_window("gpt-4"), 8_192);
        assert_eq!(model_window("gpt-3.5-turbo"), 16_385);
    }
}
