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
}
