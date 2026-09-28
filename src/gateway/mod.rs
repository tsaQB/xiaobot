pub mod whatsapp;

/// Kontrak keluaran minimum untuk kanal berbasis teks biasa.
///
/// Telegram sengaja tidak mengimplementasikan trait ini: kanal tersebut
/// mengirim blok rich, draf streaming, dan tombol interaktif yang tidak
/// dapat diwakili oleh antarmuka teks datar.
#[allow(async_fn_in_trait)]
pub trait DeliverySink: Send + Sync {
    async fn indicate_typing(&self, chat_id: i64) -> Result<(), String>;
    async fn send_text(&self, chat_id: i64, text: &str) -> Result<(), String>;
    /// Mengirim berkas (misalnya hasil tool `create_document`) sebagai
    /// lampiran asli kanal, bukan sekadar menyebut namanya di teks.
    async fn send_document(
        &self,
        chat_id: i64,
        filename: &str,
        bytes: Vec<u8>,
        mime_type: &str,
    ) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    /// Log macros in the gateway must never format message content or phone
    /// identities. Unlike the old line-based `rg` guard, this scans complete
    /// macro invocations (including multi-line ones) and only inspects the
    /// interpolated arguments, so words such as "text" inside a log sentence
    /// are not false positives.
    #[test]
    fn gateway_logs_do_not_interpolate_private_values() {
        const FORBIDDEN: &[&str] = &[
            "text", "trimmed", "raw_text", "phone", "jid", "sender", "owner", "number", "content",
            "caption", "chat_jid",
        ];
        let sources = [
            ("client.rs", include_str!("whatsapp/client.rs")),
            ("delivery.rs", include_str!("whatsapp/delivery.rs")),
            ("mapper.rs", include_str!("whatsapp/mapper.rs")),
            ("mod.rs", include_str!("whatsapp/mod.rs")),
        ];
        for (name, source) in sources {
            for macro_name in ["info!(", "warn!(", "error!("] {
                let mut cursor = 0;
                while let Some(offset) = source[cursor..].find(macro_name) {
                    let start = cursor + offset + macro_name.len();
                    let mut depth = 1usize;
                    let mut end = start;
                    for (index, ch) in source[start..].char_indices() {
                        match ch {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = start + index;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    let invocation = &source[start..end];
                    let interpolated = interpolated_names(invocation);
                    for name_used in &interpolated {
                        let lowered = name_used.to_ascii_lowercase();
                        assert!(
                            !FORBIDDEN.iter().any(|word| lowered.contains(word)),
                            "{name}: `{macro_name}...)` logs `{name_used}`, which may carry private data"
                        );
                    }
                    cursor = end.max(start);
                }
            }
        }
    }

    /// Identifiers used as format arguments: `{name}` captures inside string
    /// literals and bare expressions after the format string.
    fn interpolated_names(invocation: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut in_string = false;
        let mut escaped = false;
        let mut brace: Option<String> = None;
        let mut outside = String::new();
        for ch in invocation.chars() {
            if in_string {
                if escaped {
                    escaped = false;
                    continue;
                }
                match ch {
                    '\\' => escaped = true,
                    '"' => in_string = false,
                    '{' => brace = Some(String::new()),
                    '}' => {
                        if let Some(captured) = brace.take() {
                            let ident: String = captured
                                .chars()
                                .take_while(|c| c.is_alphanumeric() || *c == '_')
                                .collect();
                            if !ident.is_empty() && !ident.chars().all(|c| c.is_ascii_digit()) {
                                names.push(ident);
                            }
                        }
                    }
                    other => {
                        if let Some(captured) = brace.as_mut() {
                            captured.push(other);
                        }
                    }
                }
            } else if ch == '"' {
                in_string = true;
            } else {
                outside.push(ch);
            }
        }
        for token in outside.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
            if !token.is_empty() && !token.chars().all(|c| c.is_ascii_digit()) {
                names.push(token.to_string());
            }
        }
        names
    }
}
