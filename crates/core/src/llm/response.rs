/// Raw text of an LLM response together with its content/reasoning sizes,
/// used by the parsing code to build informative errors.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub raw: String,
    pub content_chars: usize,
    pub reasoning_chars: usize,
}

impl LlmResponse {
    pub fn empty() -> Self {
        Self {
            raw: String::new(),
            content_chars: 0,
            reasoning_chars: 0,
        }
    }

    pub fn from_text(text: String) -> Self {
        let chars = text.chars().count();
        Self {
            raw: text,
            content_chars: chars,
            reasoning_chars: 0,
        }
    }
}
