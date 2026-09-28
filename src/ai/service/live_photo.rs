//! The `send_live_photo` tool (Bot API 10.0 `sendLivePhoto`): a still photo
//! with a short motion clip. Telegram does not accept live photos by URL, so
//! Xiao downloads both files through the SSRF-safe fetcher and uploads them.

use crate::ai::tools::SendLivePhotoArgs;
use crate::bot::client::TelegramBotClient;

/// Telegram's limits for the motion clip.
const MAX_CLIP_BYTES: usize = 10 * 1024 * 1024;
const MAX_CLIP_SECS: f64 = 10.0;
/// Bound on the still photo download.
const MAX_PHOTO_BYTES: usize = 10 * 1024 * 1024;
/// Start of the tool result for a live photo that reached the chat.
pub(super) const LIVE_PHOTO_SENT: &str = "Live photo berhasil dikirim ke obrolan.";

/// Reads the duration of an MP4/MOV file from its `moov/mvhd` box. `None`
/// when the file is not a readable MP4.
fn mp4_duration_secs(bytes: &[u8]) -> Option<f64> {
    fn find_box<'a>(mut data: &'a [u8], wanted: &[u8; 4]) -> Option<&'a [u8]> {
        while data.len() >= 8 {
            let size = u32::from_be_bytes(data[..4].try_into().ok()?) as usize;
            let kind = &data[4..8];
            let (header, size) = match size {
                1 => {
                    let large = u64::from_be_bytes(data.get(8..16)?.try_into().ok()?);
                    (16, usize::try_from(large).ok()?)
                }
                0 => (8, data.len()),
                size => (8, size),
            };
            if size < header || size > data.len() {
                return None;
            }
            if kind == wanted {
                return Some(&data[header..size]);
            }
            data = &data[size..];
        }
        None
    }
    let mvhd = find_box(find_box(bytes, b"moov")?, b"mvhd")?;
    let (timescale, duration) = match *mvhd.first()? {
        0 => (
            u32::from_be_bytes(mvhd.get(12..16)?.try_into().ok()?),
            u64::from(u32::from_be_bytes(mvhd.get(16..20)?.try_into().ok()?)),
        ),
        1 => (
            u32::from_be_bytes(mvhd.get(20..24)?.try_into().ok()?),
            u64::from_be_bytes(mvhd.get(24..32)?.try_into().ok()?),
        ),
        _ => return None,
    };
    (timescale > 0).then(|| duration as f64 / f64::from(timescale))
}

/// Checks the downloaded clip before anything else is fetched: it must be
/// a readable MP4 of at most ten seconds.
fn check_clip(video: &[u8]) -> Result<(), String> {
    match mp4_duration_secs(video) {
        None => Err(
            "Gagal mengirim live photo: video_url bukan video MP4 yang dapat dibaca.".to_string(),
        ),
        Some(secs) if secs > MAX_CLIP_SECS + 0.05 => Err(format!(
            "Gagal mengirim live photo: video berdurasi {secs:.1} detik, padahal maksimal {MAX_CLIP_SECS:.0} detik."
        )),
        Some(_) => Ok(()),
    }
}

/// Uploads a checked clip and its photo. Returns the tool result.
async fn upload(
    bot: &TelegramBotClient,
    chat_id: i64,
    reply_to_message_id: Option<i64>,
    video: Vec<u8>,
    photo: Vec<u8>,
    caption: Option<&str>,
) -> String {
    match bot
        .send_live_photo(chat_id, video, photo, caption, reply_to_message_id)
        .await
    {
        Ok(_) => format!("{LIVE_PHOTO_SENT} Jangan sematkan tag media untuknya."),
        Err(err) => format!("Gagal mengirim live photo ke Telegram: {err}"),
    }
}

/// Runs one `send_live_photo` tool call.
pub(super) async fn run_send_live_photo(
    bot: Option<&TelegramBotClient>,
    chat_id: i64,
    reply_to_message_id: Option<i64>,
    arguments: &str,
) -> String {
    let mut args = match serde_json::from_str::<SendLivePhotoArgs>(arguments) {
        Ok(args) => args,
        Err(err) => return format!("Format argumen live photo tidak valid: {err}"),
    };
    args.sanitize();
    if let Err(err) = args.validate() {
        return format!("Validasi live photo gagal: {err}");
    }
    let Some(bot) = bot else {
        return "Live photo hanya dapat dikirim di Telegram; berikan tautan videonya saja."
            .to_string();
    };
    let Some((video, _, _)) = bot
        .download_media_bytes(&args.video_url, MAX_CLIP_BYTES)
        .await
    else {
        return "Gagal mengunduh video live photo (tidak dapat diakses, bukan alamat publik, atau lebih dari 10 MB).".to_string();
    };
    // A clip Telegram would refuse is rejected before the photo is fetched.
    if let Err(err) = check_clip(&video) {
        return err;
    }
    let Some((photo, _, _)) = bot
        .download_media_bytes(&args.photo_url, MAX_PHOTO_BYTES)
        .await
    else {
        return "Gagal mengunduh foto live photo (tidak dapat diakses, bukan alamat publik, atau terlalu besar).".to_string();
    };
    upload(
        bot,
        chat_id,
        reply_to_message_id,
        video,
        photo,
        args.caption.as_deref(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::test_support::FakeTelegram;

    /// A minimal MP4: `ftyp`, then `moov` holding a version-0 `mvhd`.
    fn mp4(timescale: u32, duration: u32) -> Vec<u8> {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&timescale.to_be_bytes());
        mvhd[16..20].copy_from_slice(&duration.to_be_bytes());
        let boxed = |kind: &[u8; 4], body: &[u8]| {
            let mut out = u32::try_from(body.len() + 8)
                .expect("small box")
                .to_be_bytes()
                .to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            out
        };
        let mut file = boxed(b"ftyp", b"isom\0\0\0\0isom");
        file.extend(boxed(b"moov", &boxed(b"mvhd", &mvhd)));
        file
    }

    const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0, 0];

    #[test]
    fn reads_the_clip_duration() {
        assert_eq!(mp4_duration_secs(&mp4(1000, 3_500)), Some(3.5));
        assert_eq!(mp4_duration_secs(b"bukan video"), None);
    }

    #[tokio::test]
    async fn uploads_the_clip_and_photo_as_one_live_photo() {
        let fake = FakeTelegram::always_ok().await;
        let clip = mp4(600, 1_800);
        assert_eq!(check_clip(&clip), Ok(()));
        let result = upload(
            &fake.client,
            5,
            Some(9),
            clip,
            JPEG.to_vec(),
            Some("Ombak pagi"),
        )
        .await;
        assert!(result.starts_with(LIVE_PHOTO_SENT), "{result}");
        let request = &fake.requests()[0];
        assert_eq!(request.method, "sendLivePhoto");
        for field in [
            "name=\"live_photo\"",
            "name=\"photo\"",
            "Ombak pagi",
            "\"message_id\":9",
        ] {
            assert!(request.raw_body.contains(field), "missing {field}");
        }
    }

    #[test]
    fn clips_longer_than_ten_seconds_or_unreadable_are_refused() {
        let too_long = check_clip(&mp4(1000, 12_000)).expect_err("12 s clip refused");
        assert!(too_long.contains("12.0 detik"), "{too_long}");
        assert!(check_clip(b"bukan video").is_err());
    }

    #[tokio::test]
    async fn outside_telegram_the_tool_explains_itself() {
        let result = run_send_live_photo(
            None,
            5,
            None,
            r#"{"video_url": "https://example.com/a.mp4", "photo_url": "https://example.com/a.jpg"}"#,
        )
        .await;
        assert!(
            result.contains("hanya dapat dikirim di Telegram"),
            "{result}"
        );

        let invalid =
            run_send_live_photo(None, 5, None, r#"{"video_url": "a.mp4", "photo_url": "x"}"#).await;
        assert!(
            invalid.starts_with("Validasi live photo gagal"),
            "{invalid}"
        );
    }
}
