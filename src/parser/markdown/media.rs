use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

use super::parse_inline;
use crate::bot::models::{Location, RichBlock, RichBlockCaption};

pub(crate) fn normalize_inline_media_label(label: &str) -> String {
    let t = label.trim();
    if let Some((tag, rest)) = t.split_once(':') {
        let tag_clean = tag.trim().to_lowercase();
        let rest_clean = rest.trim();
        let name = if rest_clean.is_empty() {
            tag.trim()
        } else {
            rest_clean
        };
        match tag_clean.as_str() {
            "photo" | "foto" | "image" | "img" | "gambar" | "picture" | "pic" => {
                return format!("📷 {name}");
            }
            "video" | "vid" => {
                return format!("🎬 {name}");
            }
            "audio" | "musik" | "music" | "lagu" | "song" => {
                return format!("🎵 {name}");
            }
            "voice" | "voicenote" | "voice_note" | "suara" | "rekaman" | "vn" => {
                return format!("🎙️ {name}");
            }
            "animation" | "animasi" | "gif" => {
                return format!("🎞️ {name}");
            }
            "document" | "dokumen" | "doc" | "file" | "berkas" => {
                return format!("📄 {name}");
            }
            "map" | "location" | "lokasi" | "peta" | "geo" => {
                return format!("📍 {name}");
            }
            _ => {}
        }
    }
    t.to_string()
}

pub(crate) fn is_border_line(line: &str) -> bool {
    let s = line.trim();
    if s.is_empty() {
        return true;
    }
    s.chars()
        .all(|c| "┌╔┏┬┰├┝┼╂└╚┗┴┸┤┥─━═+-=_ \t┐┘┒┙╗╝┚┖┓┛│|║┃".contains(c))
}

pub(crate) fn parse_coords_pair(text: &str) -> Option<(f64, f64, Option<i32>)> {
    let clean = text.trim().trim_matches(['(', ')', '[', ']']);
    let clean = clean.strip_prefix("geo:").unwrap_or(clean);
    let (coords_part, zoom_part) = if let Some((c, z)) = clean.split_once("?z=") {
        (c, z.parse::<i32>().ok())
    } else if let Some((c, z)) = clean.split_once("zoom=") {
        (c.trim_end_matches([',', ' ']), z.parse::<i32>().ok())
    } else {
        (clean, None)
    };
    let parts: Vec<&str> = coords_part.split(',').map(str::trim).collect();
    if parts.len() >= 2 {
        let lat = parts[0].parse::<f64>().ok()?;
        let lon = parts[1].parse::<f64>().ok()?;
        let zoom = zoom_part.or_else(|| {
            parts.get(2).and_then(|z| {
                z.strip_prefix("zoom=")
                    .unwrap_or(z)
                    .trim()
                    .parse::<i32>()
                    .ok()
            })
        });
        return Some((lat, lon, zoom));
    }
    None
}

pub(crate) fn try_parse_map_block(line: &str) -> Option<RichBlock> {
    let s = line.trim();
    let s_clean = s.trim_end_matches(['.', ',', ';', ':']);

    // Tag based: [map: ...], [location: ...], [lokasi: ...], [peta: ...], [geo: ...]
    let candidate = s_clean.strip_prefix('!').unwrap_or(s_clean);
    if candidate.starts_with('[') {
        if let Some(bracket_end) = candidate.find(']') {
            let tag_part = &candidate[1..bracket_end];
            if let Some((tag_name, label)) = tag_part.split_once(':') {
                let t = tag_name.trim().to_lowercase();
                if matches!(t.as_str(), "map" | "location" | "lokasi" | "peta" | "geo") {
                    let right = candidate[bracket_end + 1..].trim();
                    let right_clean = right.trim_end_matches(['.', ',', ';', ':', ' ']);
                    let coords_source = if let Some(inner) = right_clean
                        .strip_prefix('(')
                        .and_then(|r| r.strip_suffix(')'))
                    {
                        let link = inner
                            .trim()
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .trim();
                        if link.starts_with("http") {
                            link.split("?q=").nth(1).unwrap_or(link)
                        } else {
                            link
                        }
                    } else {
                        label.trim()
                    };

                    if let Some((lat, lon, zoom)) = parse_coords_pair(coords_source) {
                        return RichBlock::map_coords(lat, lon, zoom).ok();
                    }
                }
            }
        }
    }

    // ![map](geo:...) or ![location](geo:...)
    if let Some(geo) = s_clean
        .strip_prefix("![map](geo:")
        .or_else(|| s_clean.strip_prefix("![location](geo:"))
        .or_else(|| s_clean.strip_prefix("![lokasi](geo:"))
        .and_then(|r| r.strip_suffix(')'))
    {
        if let Some((lat, lon, zoom)) = parse_coords_pair(geo) {
            return RichBlock::map_coords(lat, lon, zoom).ok();
        }
    }

    // <tg-map lat="..." lon="..." zoom="..."/>
    if let Some(rest) = s.strip_prefix("<tg-map") {
        let lat_s = extract_html_attribute(rest, "lat").unwrap_or("");
        let lon_s = extract_html_attribute(rest, "lon").unwrap_or("");
        let zoom_s = extract_html_attribute(rest, "zoom");
        let lat = lat_s.parse::<f64>().ok()?;
        let lon = lon_s.parse::<f64>().ok()?;
        let zoom = match zoom_s {
            Some(z) => Some(z.parse::<i32>().ok()?),
            None => None,
        };
        return RichBlock::map_coords(lat, lon, zoom).ok();
    }

    None
}

pub(crate) fn split_bracket_and_parenthesis(text: &str) -> Option<(&str, &str)> {
    let (left, right) = text.split_once(']')?;
    let right = right.trim();
    let right_cleaned = right.trim_end_matches(['.', ',', ';', ':', ' ']);
    let inner_right = right_cleaned.strip_prefix('(')?.strip_suffix(')')?.trim();
    let inner_right = inner_right
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    Some((left.trim(), inner_right))
}

pub(crate) fn classify_media_tag(tag: &str) -> Option<&'static str> {
    let t = tag.trim().to_lowercase();
    match t.as_str() {
        "photo" | "foto" | "image" | "img" | "gambar" | "picture" | "pic" => Some("photo"),
        "video" | "vid" => Some("video"),
        "audio" | "musik" | "music" | "lagu" | "song" => Some("audio"),
        "voice" | "voicenote" | "voice_note" | "suara" | "rekaman" | "vn" => Some("voice"),
        "animation" | "animasi" | "gif" => Some("animation"),
        "collage" | "kolase" | "gallery" | "galeri" | "album" => Some("collage"),
        "slideshow" | "slide" => Some("slideshow"),
        "document" | "dokumen" | "doc" | "file" | "berkas" => Some("document"),
        "map" | "location" | "lokasi" | "peta" | "geo" => Some("map"),
        _ => None,
    }
}

pub fn is_streaming_web_video(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("youtube.com/")
        || lower.contains("youtu.be/")
        || lower.contains("vimeo.com/")
        || lower.contains("dailymotion.com/")
        || lower.contains("twitch.tv/")
        || lower.contains("tiktok.com/")
        || lower.contains("bilibili.com/")
        || lower.contains("instagram.com/reel/")
        || lower.contains("facebook.com/watch")
        || lower.contains("streamable.com/")
        || lower.contains("loom.com/")
}

pub fn is_streaming_web_audio(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("spotify.com/")
        || lower.contains("soundcloud.com/")
        || lower.contains("music.apple.com/")
        || lower.contains("podcasts.apple.com/")
        || lower.contains("podbean.com/")
        || lower.contains("anchor.fm/")
        || lower.contains("mixcloud.com/")
        || lower.contains("bandcamp.com/")
        || lower.contains("audiomack.com/")
}

pub fn is_unsupported_image_format(url: &str) -> bool {
    let clean = url.split('?').next().unwrap_or(url).to_lowercase();
    clean.ends_with(".svg")
        || clean.ends_with(".bmp")
        || clean.ends_with(".tiff")
        || clean.ends_with(".tif")
        || clean.ends_with(".ico")
        || clean.ends_with(".heic")
        || clean.ends_with(".avif")
        || clean.ends_with(".html")
        || clean.ends_with(".htm")
        || clean.ends_with(".php")
        || clean.contains("imgur.com/a/")
        || clean.contains("imgur.com/gallery/")
        || clean.contains("flickr.com/photos/")
        || clean.contains("pinterest.com/pin/")
}

pub(crate) fn extract_html_attribute<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let mut cursor = 0;
    // ASCII-only lowercasing keeps byte offsets identical to `tag`, so indexes
    // found in `tag_lower` are always valid char boundaries in `tag`. Unicode
    // lowercasing (e.g. `İ` -> `i̇`) changes byte lengths and made the slice
    // below panic on crafted attribute values.
    let attr_lower = attr.to_ascii_lowercase();
    let tag_lower = tag.to_ascii_lowercase();
    while let Some(idx) = tag_lower[cursor..].find(&attr_lower) {
        let pos = cursor + idx;
        let after_attr = &tag[pos + attr.len()..];
        // Ensure word boundary before attr
        if pos > 0 {
            let prev = tag.as_bytes()[pos - 1];
            if prev.is_ascii_alphanumeric() || prev == b'-' || prev == b'_' {
                cursor = pos + attr.len();
                continue;
            }
        }
        let trimmed_after = after_attr.trim_start();
        if let Some(rest) = trimmed_after.strip_prefix('=') {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix('"') {
                if let Some(end) = val.find('"') {
                    return Some(&val[..end]);
                }
            } else if let Some(val) = rest.strip_prefix('\'') {
                if let Some(end) = val.find('\'') {
                    return Some(&val[..end]);
                }
            } else {
                let end = rest.find([' ', '>', '/', '\t', '\n']).unwrap_or(rest.len());
                if end > 0 {
                    return Some(&rest[..end]);
                }
            }
        }
        cursor = pos + attr.len();
    }
    None
}

pub(crate) fn format_media_fallback_paragraph(
    label: &str,
    link: &str,
    default_label: &str,
    emoji: &str,
) -> RichBlock {
    let cap_text = if label.is_empty() {
        default_label
    } else {
        label
    };
    RichBlock::Paragraph {
        text: parse_inline(&format!("{emoji} [{cap_text}]({link})")),
    }
}

pub(crate) fn parse_multi_media_list_block(
    link: &str,
    label: &str,
    caption: Option<RichBlockCaption>,
    is_slideshow: bool,
) -> Option<RichBlock> {
    let urls: Vec<&str> = link
        .split(',')
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .collect();
    let mut valid_blocks = Vec::new();
    let mut fallback_urls = Vec::new();
    for u in urls {
        if is_unsupported_image_format(u) || is_streaming_web_video(u) || is_streaming_web_audio(u)
        {
            fallback_urls.push(u);
        } else {
            valid_blocks.push(json!({"type": "photo", "photo": {"type": "photo", "media": u}}));
        }
    }
    if valid_blocks.len() >= 2 {
        if is_slideshow {
            Some(RichBlock::Slideshow {
                blocks: valid_blocks,
                caption,
            })
        } else {
            Some(RichBlock::Collage {
                blocks: valid_blocks,
                caption,
            })
        }
    } else if valid_blocks.len() == 1 {
        let first = valid_blocks
            .pop()
            .expect("guaranteed single element in valid_blocks");
        let photo_val = first.get("photo").cloned().unwrap_or(first);
        Some(RichBlock::Photo {
            photo: photo_val,
            caption,
        })
    } else {
        let default_title = if is_slideshow {
            "Slideshow"
        } else {
            "Galeri Foto"
        };
        let cap_text = if label.is_empty() {
            default_title
        } else {
            label
        };
        let item_prefix = if is_slideshow { "Slide" } else { "Foto" };
        let mut text_parts = format!("🖼️ [{cap_text}]: ");
        for (idx, u) in fallback_urls.into_iter().enumerate() {
            if idx > 0 {
                text_parts.push_str(" • ");
            }
            text_parts.push_str(&format!("[{item_prefix} #{}]({u})", idx + 1));
        }
        Some(RichBlock::Paragraph {
            text: parse_inline(&text_parts),
        })
    }
}

pub(crate) fn try_parse_doc_block(line: &str) -> Option<RichBlock> {
    let s = line.trim();
    let s_clean = s.trim_end_matches(['.', ',', ';', ':']);

    if s_clean.starts_with('[') || s_clean.starts_with("![") {
        let candidate = s_clean.strip_prefix('!').unwrap_or(s_clean);
        if let Some(bracket_end) = candidate.find(']') {
            let tag_part = &candidate[1..bracket_end];
            if let Some((tag_name, name)) = tag_part.split_once(':') {
                let t = tag_name.trim().to_lowercase();
                if matches!(
                    t.as_str(),
                    "document" | "dokumen" | "doc" | "file" | "berkas"
                ) {
                    let right = candidate[bracket_end + 1..].trim();
                    let right_clean = right.trim_end_matches(['.', ',', ';', ':', ' ']);
                    if let Some(inner) = right_clean
                        .strip_prefix('(')
                        .and_then(|r| r.strip_suffix(')'))
                    {
                        let link = inner
                            .trim()
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .trim();
                        let name = name.trim();
                        return Some(RichBlock::Document {
                            document: json!({"type": "document", "media": link}),
                            caption: (!name.is_empty())
                                .then(|| RichBlockCaption::new(parse_inline(name))),
                        });
                    }
                }
            }
        }
    }

    if let Some(rest) = s.strip_prefix("<tg-document") {
        let trimmed = rest.trim().trim_end_matches('>').trim_end_matches('/');
        let link = trimmed
            .split("src=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .or_else(|| {
                trimmed
                    .split("src='")
                    .nth(1)
                    .and_then(|s| s.split('\'').next())
            })
            .unwrap_or("");
        let name = trimmed
            .split("name=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .or_else(|| {
                trimmed
                    .split("name='")
                    .nth(1)
                    .and_then(|s| s.split('\'').next())
            })
            .unwrap_or("");
        if !link.is_empty() {
            return Some(RichBlock::Document {
                document: json!({"type": "document", "media": link}),
                caption: (!name.is_empty()).then(|| RichBlockCaption::new(parse_inline(name))),
            });
        }
    }
    None
}

pub(crate) fn try_parse_media_block(line: &str) -> Option<RichBlock> {
    let s = line.trim();
    let s_clean = s.trim_end_matches(['.', ',', ';', ':']);

    // 1. Bracketed media tags [photo: ...] or ![photo: ...]
    let candidate = s_clean.strip_prefix('!').unwrap_or(s_clean).trim_start();
    if candidate.starts_with('[') {
        if let Some(bracket_end) = candidate.find(']') {
            let tag_part = &candidate[1..bracket_end];
            if let Some((tag_name, label)) = tag_part.split_once(':') {
                if let Some(kind) = classify_media_tag(tag_name) {
                    let label = label.trim();
                    let right = candidate[bracket_end + 1..].trim();
                    let right_clean = right.trim_end_matches(['.', ',', ';', ':', ' ']);

                    // Map without parenthesis: [map: -6.2, 106.8]
                    if kind == "map" && right_clean.is_empty() {
                        if let Some((lat, lon, zoom)) = parse_coords_pair(label) {
                            return Some(RichBlock::Map {
                                location: Location {
                                    latitude: lat,
                                    longitude: lon,
                                    horizontal_accuracy: None,
                                },
                                zoom,
                                width: None,
                                height: None,
                            });
                        }
                    }

                    if let Some(inner_right) = right_clean
                        .strip_prefix('(')
                        .and_then(|r| r.strip_suffix(')'))
                    {
                        let link = inner_right
                            .trim()
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .trim();
                        let caption =
                            (!label.is_empty()).then(|| RichBlockCaption::new(parse_inline(label)));

                        match kind {
                            "photo" => {
                                if is_streaming_web_video(link) {
                                    return Some(format_media_fallback_paragraph(
                                        label,
                                        link,
                                        "Tonton Video",
                                        "🎬",
                                    ));
                                }
                                if is_streaming_web_audio(link) {
                                    return Some(format_media_fallback_paragraph(
                                        label,
                                        link,
                                        "Dengarkan Audio",
                                        "🎵",
                                    ));
                                }
                                if is_unsupported_image_format(link) {
                                    return Some(format_media_fallback_paragraph(
                                        label,
                                        link,
                                        "Lihat Foto",
                                        "🖼️",
                                    ));
                                }
                                return Some(RichBlock::Photo {
                                    photo: json!({"type": "photo", "media": link}),
                                    caption,
                                });
                            }
                            "video" => {
                                if is_streaming_web_video(link) {
                                    return Some(format_media_fallback_paragraph(
                                        label,
                                        link,
                                        "Tonton Video",
                                        "🎬",
                                    ));
                                }
                                return Some(RichBlock::Video {
                                    video: json!({"type": "video", "media": link}),
                                    caption,
                                });
                            }
                            "audio" => {
                                if is_streaming_web_audio(link) {
                                    return Some(format_media_fallback_paragraph(
                                        label,
                                        link,
                                        "Dengarkan Audio",
                                        "🎵",
                                    ));
                                }
                                return Some(RichBlock::Audio {
                                    audio: json!({"type": "audio", "media": link}),
                                    caption,
                                });
                            }
                            "voice" => {
                                return Some(RichBlock::VoiceNote {
                                    voice_note: json!({"type": "voice_note", "media": link}),
                                    caption,
                                });
                            }
                            "animation" => {
                                return Some(RichBlock::Animation {
                                    animation: json!({"type": "animation", "media": link}),
                                    caption,
                                });
                            }
                            "document" => {
                                return Some(RichBlock::Document {
                                    document: json!({"type": "document", "media": link}),
                                    caption,
                                });
                            }
                            "collage" => {
                                return parse_multi_media_list_block(link, label, caption, false);
                            }
                            "slideshow" => {
                                return parse_multi_media_list_block(link, label, caption, true);
                            }
                            "map" => {
                                let coords_str = if link.starts_with("http") {
                                    link.split("?q=").nth(1).unwrap_or(link)
                                } else {
                                    link
                                };
                                if let Some((lat, lon, zoom)) = parse_coords_pair(coords_str) {
                                    return Some(RichBlock::Map {
                                        location: Location {
                                            latitude: lat,
                                            longitude: lon,
                                            horizontal_accuracy: None,
                                        },
                                        zoom,
                                        width: None,
                                        height: None,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    // 2. Plain Markdown image syntax ![alt](url)
    if let Some(rest) = s_clean.strip_prefix("![") {
        let lower = s_clean.to_lowercase();
        if !lower.starts_with("![map")
            && !lower.starts_with("![location")
            && !lower.starts_with("![lokasi")
            && !lower.starts_with("![peta")
        {
            if let Some((alt, link)) = split_bracket_and_parenthesis(rest) {
                if link.starts_with("http://")
                    || link.starts_with("https://")
                    || link.starts_with("tg://")
                {
                    if is_streaming_web_video(link) {
                        return Some(format_media_fallback_paragraph(
                            alt,
                            link,
                            "Tonton Video",
                            "🎬",
                        ));
                    }
                    if is_streaming_web_audio(link) {
                        return Some(format_media_fallback_paragraph(
                            alt,
                            link,
                            "Dengarkan Audio",
                            "🎵",
                        ));
                    }
                    if is_unsupported_image_format(link) {
                        return Some(format_media_fallback_paragraph(
                            alt,
                            link,
                            "Lihat Foto",
                            "🖼️",
                        ));
                    }

                    let clean_url = link.split('?').next().unwrap_or(link);
                    let lower_link = clean_url.to_lowercase();
                    let caption =
                        (!alt.is_empty()).then(|| RichBlockCaption::new(parse_inline(alt)));
                    if lower_link.ends_with(".mp4")
                        || lower_link.ends_with(".webm")
                        || lower_link.ends_with(".mov")
                    {
                        return Some(RichBlock::Video {
                            video: json!({"type": "video", "media": link}),
                            caption,
                        });
                    } else if lower_link.ends_with(".mp3")
                        || lower_link.ends_with(".ogg")
                        || lower_link.ends_with(".wav")
                        || lower_link.ends_with(".m4a")
                    {
                        return Some(RichBlock::Audio {
                            audio: json!({"type": "audio", "media": link}),
                            caption,
                        });
                    } else if lower_link.ends_with(".gif") {
                        return Some(RichBlock::Animation {
                            animation: json!({"type": "animation", "media": link}),
                            caption,
                        });
                    } else {
                        return Some(RichBlock::Photo {
                            photo: json!({"type": "photo", "media": link}),
                            caption,
                        });
                    }
                }
            }
        }
    }

    // 3. Telegram native HTML media tags: <tg-photo ...>, <tg-video ...>, <tg-audio ...>, <img ...>, <audio ...>
    if s.starts_with("<tg-photo")
        || s.starts_with("<tg-video")
        || s.starts_with("<tg-audio")
        || s.starts_with("<img")
        || s.starts_with("<audio")
    {
        if let Some(block) = try_parse_html_media_tag(s) {
            return Some(block);
        }
    }

    None
}

pub(crate) fn try_parse_html_media_tag(tag: &str) -> Option<RichBlock> {
    let s = tag.trim();
    let src = extract_html_attribute(s, "src").unwrap_or("");
    if src.is_empty() {
        return None;
    }
    let inner_text = s
        .split('>')
        .nth(1)
        .and_then(|t| t.split("</").next())
        .map(str::trim)
        .filter(|t| !t.is_empty());

    if s.starts_with("<tg-photo") || s.starts_with("<img") {
        let cap_attr = extract_html_attribute(s, "caption")
            .or_else(|| extract_html_attribute(s, "alt"))
            .or_else(|| extract_html_attribute(s, "title"));
        let caption_text = cap_attr.or(inner_text).unwrap_or("");
        let caption =
            (!caption_text.is_empty()).then(|| RichBlockCaption::new(parse_inline(caption_text)));

        if is_streaming_web_video(src) {
            return Some(format_media_fallback_paragraph(
                caption_text,
                src,
                "Tonton Video",
                "🎬",
            ));
        }
        if is_streaming_web_audio(src) {
            return Some(format_media_fallback_paragraph(
                caption_text,
                src,
                "Dengarkan Audio",
                "🎵",
            ));
        }
        if is_unsupported_image_format(src) {
            return Some(format_media_fallback_paragraph(
                caption_text,
                src,
                "Lihat Foto",
                "🖼️",
            ));
        }
        return Some(RichBlock::Photo {
            photo: json!({"type": "photo", "media": src}),
            caption,
        });
    }

    if s.starts_with("<tg-video") {
        let cap_attr =
            extract_html_attribute(s, "caption").or_else(|| extract_html_attribute(s, "title"));
        let caption_text = cap_attr.or(inner_text).unwrap_or("");
        let caption =
            (!caption_text.is_empty()).then(|| RichBlockCaption::new(parse_inline(caption_text)));

        if is_streaming_web_video(src) {
            return Some(format_media_fallback_paragraph(
                caption_text,
                src,
                "Tonton Video",
                "🎬",
            ));
        }
        return Some(RichBlock::Video {
            video: json!({"type": "video", "media": src}),
            caption,
        });
    }

    if s.starts_with("<tg-audio") || s.starts_with("<audio") {
        let cap_attr = extract_html_attribute(s, "caption");
        let caption_text = cap_attr.or(inner_text).unwrap_or("");
        let caption =
            (!caption_text.is_empty()).then(|| RichBlockCaption::new(parse_inline(caption_text)));

        if is_streaming_web_audio(src) {
            return Some(format_media_fallback_paragraph(
                caption_text,
                src,
                "Dengarkan Audio",
                "🎵",
            ));
        }
        let title = extract_html_attribute(s, "title");
        let performer = extract_html_attribute(s, "performer");
        let mut audio_obj = json!({"type": "audio", "media": src});
        if let Some(t) = title {
            audio_obj["title"] = Value::String(t.to_string());
        }
        if let Some(p) = performer {
            audio_obj["performer"] = Value::String(p.to_string());
        }
        return Some(RichBlock::Audio {
            audio: audio_obj,
            caption,
        });
    }

    None
}

pub(crate) fn try_parse_container_media_block(
    lines: &[String],
    start_idx: usize,
) -> Option<(RichBlock, usize)> {
    let first_line = lines[start_idx].trim();
    let is_html = first_line.starts_with('<');
    let is_slideshow = first_line.to_lowercase().contains("slideshow");

    let close_tag = if is_html {
        if is_slideshow {
            "</tg-slideshow>"
        } else {
            "</tg-collage>"
        }
    } else if is_slideshow {
        "[/slideshow]"
    } else if first_line.to_lowercase().contains("kolase") {
        "[/kolase]"
    } else {
        "[/collage]"
    };

    let mut collected = Vec::new();
    let mut i = start_idx;
    let n = lines.len();

    if is_html && first_line.contains(close_tag) {
        collected.push(first_line.to_string());
        i += 1;
    } else {
        while i < n {
            let line = lines[i].trim();
            collected.push(line.to_string());
            i += 1;
            if line.contains(close_tag) {
                break;
            }
        }
    }

    let full_content = collected.join("\n");
    let caption_text = if is_html {
        extract_html_attribute(first_line, "caption")
            .or_else(|| extract_html_attribute(first_line, "title"))
            .or_else(|| extract_html_attribute(first_line, "alt"))
    } else {
        first_line
            .strip_prefix('[')
            .and_then(|s| s.split_once(']'))
            .map(|(tag_part, _)| tag_part)
            .and_then(|t| t.split_once(':'))
            .map(|(_, cap)| cap.trim())
            .filter(|c| !c.is_empty())
    };

    let caption = caption_text.map(|c| RichBlockCaption::new(parse_inline(c)));

    static RE_MEDIA_SRC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)(?:src=["']([^"']+)["']|!?\[[^\]]*\]\(([^)]+)\)|https?://[^\s"'<>()]+)"#)
            .expect("valid static regex")
    });
    static RE_CHILD_TAG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?is)<(?:img|tg-photo|video|tg-video)\s+[^>]*?/?>"#)
            .expect("valid static regex")
    });

    let mut sub_blocks = Vec::new();
    // First try tag-based extraction to preserve per-image alt/caption
    for tag_mat in RE_CHILD_TAG.find_iter(&full_content) {
        let tag_str = tag_mat.as_str();
        let src = extract_html_attribute(tag_str, "src").unwrap_or("");
        let clean_url = src.trim_matches(['"', '\'', '<', '>']);
        if clean_url.starts_with("http://")
            || clean_url.starts_with("https://")
            || clean_url.starts_with("tg://")
            || clean_url.starts_with("attach://")
        {
            let lower = clean_url.to_lowercase();
            let is_vid = lower.ends_with(".mp4")
                || lower.ends_with(".webm")
                || lower.ends_with(".mov")
                || tag_str.to_lowercase().starts_with("<video")
                || tag_str.to_lowercase().starts_with("<tg-video");
            let child_cap = extract_html_attribute(tag_str, "caption")
                .or_else(|| extract_html_attribute(tag_str, "alt"))
                .or_else(|| extract_html_attribute(tag_str, "title"));

            if is_vid {
                let mut vid_obj = json!({"type": "video", "media": clean_url});
                if let Some(c) = child_cap {
                    vid_obj["caption"] = Value::String(c.to_string());
                }
                sub_blocks.push(json!({"type": "video", "video": vid_obj}));
            } else if !lower.ends_with(".html")
                && !lower.ends_with(".htm")
                && !is_streaming_web_video(clean_url)
                && !is_streaming_web_audio(clean_url)
                && !is_unsupported_image_format(clean_url)
            {
                let mut photo_obj = json!({"type": "photo", "media": clean_url});
                if let Some(c) = child_cap {
                    photo_obj["caption"] = Value::String(c.to_string());
                }
                sub_blocks.push(json!({"type": "photo", "photo": photo_obj}));
            }
        }
    }

    // If no HTML child tags found, fall back to RE_MEDIA_SRC
    if sub_blocks.is_empty() {
        for caps in RE_MEDIA_SRC.captures_iter(&full_content) {
            let url = caps
                .get(1)
                .or_else(|| caps.get(2))
                .or_else(|| caps.get(0))
                .map(|m| m.as_str().trim())
                .unwrap_or("");
            let clean_url = url.trim_matches(['"', '\'', '<', '>']);
            if clean_url.starts_with("http://")
                || clean_url.starts_with("https://")
                || clean_url.starts_with("tg://")
                || clean_url.starts_with("attach://")
            {
                let lower = clean_url.to_lowercase();
                if lower.ends_with(".mp4") || lower.ends_with(".webm") || lower.ends_with(".mov") {
                    sub_blocks.push(
                        json!({"type": "video", "video": {"type": "video", "media": clean_url}}),
                    );
                } else if !lower.ends_with(".html")
                    && !lower.ends_with(".htm")
                    && !is_streaming_web_video(clean_url)
                    && !is_streaming_web_audio(clean_url)
                    && !is_unsupported_image_format(clean_url)
                {
                    sub_blocks.push(
                        json!({"type": "photo", "photo": {"type": "photo", "media": clean_url}}),
                    );
                }
            }
        }
    }

    if sub_blocks.len() >= 2 {
        if !is_slideshow && sub_blocks.len() > 10 {
            sub_blocks.truncate(10);
        }
        let block = if is_slideshow {
            RichBlock::Slideshow {
                blocks: sub_blocks,
                caption,
            }
        } else {
            RichBlock::Collage {
                blocks: sub_blocks,
                caption,
            }
        };
        Some((block, i))
    } else if sub_blocks.len() == 1 {
        let first = sub_blocks
            .pop()
            .expect("guaranteed single element in sub_blocks");
        let block = if first["type"] == "video" {
            RichBlock::Video {
                video: first["video"].clone(),
                caption,
            }
        } else {
            RichBlock::Photo {
                photo: first["photo"].clone(),
                caption,
            }
        };
        Some((block, i))
    } else {
        let cap = caption_text.unwrap_or("Galeri Media");
        let mut text = format!("🖼️ [{cap}]: ");
        let mut count = 0;
        for caps in RE_MEDIA_SRC.captures_iter(&full_content) {
            let url = caps
                .get(1)
                .or_else(|| caps.get(2))
                .or_else(|| caps.get(0))
                .map(|m| m.as_str().trim())
                .unwrap_or("");
            let clean_url = url.trim_matches(['"', '\'', '<', '>']);
            if clean_url.starts_with("http://") || clean_url.starts_with("https://") {
                if count > 0 {
                    text.push_str(" • ");
                }
                count += 1;
                text.push_str(&format!("[Tautan #{count}]({clean_url})"));
            }
        }
        if count > 0 {
            Some((
                RichBlock::Paragraph {
                    text: parse_inline(&text),
                },
                i,
            ))
        } else {
            None
        }
    }
}

pub(crate) static RE_EMBEDDED_MEDIA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(!?\[(?:photo|foto|image|img|gambar|picture|pic|video|vid|audio|musik|music|lagu|song|voice|voicenote|voice_note|suara|rekaman|vn|animation|animasi|gif|collage|kolase|gallery|galeri|album|slideshow|slide|document|dokumen|doc|file|berkas|map|location|lokasi|peta|geo)\s*:[^\]]+\](?:\s*\([^\)]+\))?[.,;:]?|!\[[^\]]*\]\s*\([^\)]+\)[.,;:]?|<tg-(?:photo|video|audio|document|map|collage|slideshow)[^>]*>|</tg-(?:photo|video|audio|document|map|collage|slideshow)>|<img[^>]*>|<audio[^>]*>|</audio>)"#
    ).expect("valid static regex")
});

pub fn isolate_embedded_media_blocks(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // Pre-sanitize multiline HTML media tags outside code blocks (replace internal newlines with space)
    static RE_MULTILINE_MEDIA_TAG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?is)<(?:tg-(?:photo|video|audio|document|map|collage|slideshow)|img|audio)\b[^>]*?>"#,
        )
        .expect("valid static regex")
    });

    let normalized_text = {
        let lines: Vec<&str> = text.split('\n').collect();
        let mut output_lines = Vec::with_capacity(lines.len());
        let mut non_code_buffer = Vec::new();
        let mut in_code_block = false;

        let flush_non_code = |buf: &mut Vec<&str>, out: &mut Vec<String>| {
            if !buf.is_empty() {
                let chunk = buf.join("\n");
                let norm = RE_MULTILINE_MEDIA_TAG
                    .replace_all(&chunk, |caps: &regex::Captures| {
                        caps[0].replace(['\r', '\n'], " ")
                    })
                    .into_owned();
                for l in norm.split('\n') {
                    out.push(l.to_string());
                }
                buf.clear();
            }
        };

        for line in lines {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                flush_non_code(&mut non_code_buffer, &mut output_lines);
                in_code_block = !in_code_block;
                output_lines.push(line.to_string());
            } else if in_code_block {
                output_lines.push(line.to_string());
            } else {
                non_code_buffer.push(line);
            }
        }
        flush_non_code(&mut non_code_buffer, &mut output_lines);
        output_lines.join("\n")
    };

    let mut output = String::with_capacity(normalized_text.len() + 64);
    let mut in_code_block = false;

    for line in normalized_text.split('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
            continue;
        }

        if in_code_block || trimmed.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
            continue;
        }

        if !RE_EMBEDDED_MEDIA.is_match(line) {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
            continue;
        }

        // Split line around matches, putting each media block on its own line
        let mut last_end = 0;
        for mat in RE_EMBEDDED_MEDIA.find_iter(line) {
            let start = mat.start();
            let end = mat.end();

            let before = line[last_end..start].trim();
            if !before.is_empty() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(before);
            }

            let matched_tag = line[start..end].trim();
            if !matched_tag.is_empty() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(matched_tag);
            }

            last_end = end;
        }

        let after = line[last_end..].trim();
        if !after.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(after);
        }
    }

    output
}
