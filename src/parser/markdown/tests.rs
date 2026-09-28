use super::*;

#[test]
fn unicode_box_table_parses_without_byte_boundary_slicing() {
    let input =
        "┌──────┬──────┐\n│ Nama │ Ikon │\n├──────┼──────┤\n│ 世界 │ 😊   │\n└──────┴──────┘";
    let blocks = parse_markdown_to_rich_blocks(input);
    assert!(blocks
        .iter()
        .any(|block| matches!(block, RichBlock::Table { .. })));
}

#[test]
fn parse_markdown_to_rich_blocks_malformed_media_tag_does_not_infinite_loop() {
    let text = "Berikut gambarnya:\n<img src=\"https://i.imgur nya:";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert!(!blocks.is_empty());
}

#[test]
fn parse_streaming_markdown_to_rich_blocks_holds_unclosed_html_media_tag() {
    let text = "Berikut gambarnya:\n<img src=\"https://i.imgur";
    let blocks = parse_streaming_markdown_to_rich_blocks(text);
    // Should parse the first stable line, while keeping unclosed <img provisional
    assert!(!blocks.is_empty());
}

#[test]
fn isolate_embedded_media_blocks_normalizes_multiline_tags_outside_code_blocks() {
    let text = "<img src=\"https://example.com/test.jpg\"\ncaption=\"Multi-line\ncaption\"/>\n\n```html\n<img src=\"https://example.com/code.jpg\"\ncaption=\"Code\nblock\"/>\n```\n\n<tg-collage\ncaption=\"Kolase\nfoto\">\n<img src=\"https://example.com/c1.jpg\"/>\n</tg-collage>\n\n<tg-slideshow\ncaption=\"Slideshow\nfoto\">\n<img src=\"https://example.com/s1.jpg\"/>\n</tg-slideshow>";
    let isolated = isolate_embedded_media_blocks(text);

    // Outside tag should have internal newlines removed
    assert!(isolated
        .contains("<img src=\"https://example.com/test.jpg\" caption=\"Multi-line caption\"/>"));

    // Inside code block tag should retain internal newlines
    assert!(
        isolated.contains("<img src=\"https://example.com/code.jpg\"\ncaption=\"Code\nblock\"/>")
    );

    // Multiline tg-collage and tg-slideshow tags should have internal newlines removed
    assert!(isolated.contains("<tg-collage caption=\"Kolase foto\">"));
    assert!(isolated.contains("<tg-slideshow caption=\"Slideshow foto\">"));
}

#[test]
fn ordered_list_preserves_native_ordering_metadata() {
    let blocks = parse_markdown_to_rich_blocks("5. lima\n6. enam");
    let RichBlock::List { items } = &blocks[0] else {
        panic!("expected list");
    };
    assert_eq!(items[0].kind.as_deref(), Some("1"));
    assert_eq!(items[0].value, Some(5));
    assert_eq!(items[1].value, Some(6));
}

#[test]
fn emoji_and_multibyte_inline_text_survive_parser() {
    let value = parse_inline("Halo █ 😊 世界 **tebal**");
    let serialized = serde_json::to_string(&value).expect("serialize value succeeds");
    assert!(serialized.contains("世界"));
    assert!(serialized.contains("😊"));
}

#[test]
fn streaming_markdown_never_exposes_provisional_serialization_markers() {
    let cases = [
        "Ini **gaya gravitasi** selesai",
        "Ini _italic_ selesai",
        "Gunakan `kode` sekarang",
        "```rust\nfn main() {}\n```",
        "### Heading tumbuh",
        "---",
        "[OpenAI](https://example.com/path)",
        "1. pertama\n2. kedua",
        "- satu\n- dua",
        "Emoji 😊 世界 **tebal**",
    ];

    for source in cases {
        let mut boundaries: Vec<usize> = source.char_indices().map(|(index, _)| index).collect();
        boundaries.push(source.len());
        boundaries.sort_unstable();
        boundaries.dedup();
        for end in boundaries.into_iter().filter(|end| *end > 0) {
            let prefix = &source[..end];
            let blocks = parse_streaming_markdown_to_rich_blocks(prefix);
            let wire = serde_json::to_string(&blocks).expect("serialize blocks succeeds");
            assert!(
                !wire.contains("**"),
                "bold marker leaked for {prefix:?}: {wire}"
            );
            assert!(
                !wire.contains("__"),
                "emphasis marker leaked for {prefix:?}: {wire}"
            );
            assert!(
                !wire.contains("```"),
                "fence marker leaked for {prefix:?}: {wire}"
            );
            assert!(
                !wire.contains("]("),
                "link serialization leaked for {prefix:?}: {wire}"
            );
            if prefix.trim().chars().all(|ch| ch == '#') {
                assert!(
                    !wire.contains('#'),
                    "heading marker leaked for {prefix:?}: {wire}"
                );
            }
            if matches!(prefix.trim(), "-" | "--") {
                assert!(
                    !wire.contains(prefix.trim()),
                    "divider marker leaked for {prefix:?}: {wire}"
                );
            }
        }
    }
}

#[test]
fn collage_slideshow_audio_voice_parse_correctly() {
    let text = "[audio: Judul Musik](https://example.com/song.mp3)

[voice: Rekaman Suara](tg://audio?id=rec1)

[collage: Galeri](url1, url2)

[slideshow: Slide](url3, url4)";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert!(blocks.iter().any(|b| matches!(b, RichBlock::Audio { .. })));
    assert!(blocks
        .iter()
        .any(|b| matches!(b, RichBlock::VoiceNote { .. })));
    assert!(blocks
        .iter()
        .any(|b| matches!(b, RichBlock::Collage { .. })));
    assert!(blocks
        .iter()
        .any(|b| matches!(b, RichBlock::Slideshow { .. })));
}

#[test]
fn media_blocks_tolerate_whitespace_between_bracket_and_parenthesis() {
    let text = "[audio: Suara Contoh] (https://upload.wikimedia.org/wikipedia/commons/c/c8/Example.ogg)\n\n[photo: Foto Indah]  (https://example.com/pic.jpg)\n\n[collage: Galeri] (url1, url2)";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 3);
    assert!(matches!(blocks[0], RichBlock::Audio { .. }));
    assert!(matches!(blocks[1], RichBlock::Photo { .. }));
    assert!(matches!(blocks[2], RichBlock::Collage { .. }));
}

#[test]
fn map_and_document_blocks_parse_correctly() {
    let text = "[map: -6.175392, 106.827153, zoom=15]

[document: Laporan.pdf](tg://document?id=laporan_1)";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert!(blocks.iter().any(|b| matches!(b, RichBlock::Map { .. })));
    assert!(blocks
        .iter()
        .any(|b| matches!(b, RichBlock::Document { .. })));
}

#[test]
fn pullquote_and_footer_parse_correctly() {
    let text = ">>> Ini adalah kutipan penting

Paragraf normal";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert!(blocks
        .iter()
        .any(|b| matches!(b, RichBlock::PullQuotation { .. })));

    let full = build_full_rich_message("Jawaban AI", Some("`⚡ 3.0s`"));
    let footer = full
        .blocks
        .iter()
        .find_map(|b| match b {
            RichBlock::Footer { text } => Some(text),
            _ => None,
        })
        .expect("footer block should exist");
    let serialized = serde_json::to_string(footer).expect("serialize footer succeeds");
    assert!(serialized.contains("3.0s"));
    assert!(serialized.contains("⚡"));
    assert!(serialized.contains("code"));
}

#[test]
fn markdown_image_and_media_is_media_check() {
    let text = "Penjelasan aurora:\n\n![Cahaya Aurora](https://picsum.photos/1000/600)\n\n[photo: Tromso](https://picsum.photos/800/600)";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 3);
    assert!(!blocks[0].is_media());
    assert!(blocks[1].is_media());
    assert!(blocks[2].is_media());
    assert!(matches!(blocks[1], RichBlock::Photo { .. }));
    assert!(matches!(blocks[2], RichBlock::Photo { .. }));
}

#[test]
fn completed_streaming_markdown_converges_to_canonical_parser() {
    let source = "## Judul\n\n**tebal** dan _miring_\n\n---\n\n1. satu\n2. dua";
    let streaming = parse_streaming_markdown_to_rich_blocks(source);
    let canonical = parse_markdown_to_rich_blocks(source);
    assert_eq!(
        serde_json::to_value(streaming).expect("serialize streaming succeeds"),
        serde_json::to_value(canonical).expect("serialize canonical succeeds")
    );
}

#[test]
fn spoiler_and_strikethrough_parse_correctly() {
    let markdown = "Info: ||rahasia besar|| dan ~~harga lama~~";
    let parsed = parse_inline(markdown);
    let serialized = serde_json::to_string(&parsed).expect("serialize parsed succeeds");
    assert!(serialized.contains(r#""type":"spoiler""#));
    assert!(serialized.contains("rahasia besar"));
    assert!(serialized.contains(r#""type":"strikethrough""#));
    assert!(serialized.contains("harga lama"));

    let html = "Tag: <tg-spoiler>kunci rahasia</tg-spoiler> dan <s>coret html</s>";
    let parsed_html = parse_inline(html);
    let serialized_html =
        serde_json::to_string(&parsed_html).expect("serialize parsed_html succeeds");
    assert!(serialized_html.contains(r#""type":"spoiler""#));
    assert!(serialized_html.contains("kunci rahasia"));
    assert!(serialized_html.contains(r#""type":"strikethrough""#));
    assert!(serialized_html.contains("coret html"));
}

#[test]
fn expandable_blockquote_parses_correctly() {
    let markdown = "**> Baris penalaran pertama\n**> Baris penalaran kedua\n**> — As-tsaqib";
    let blocks = parse_markdown_to_rich_blocks(markdown);
    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::ExpandableBlockQuotation { text, credit }) = blocks.first() else {
        panic!("expected expandable blockquote");
    };
    let text_str = serde_json::to_string(text).expect("serialize text succeeds");
    assert!(text_str.contains("Baris penalaran pertama"));
    assert!(text_str.contains("Baris penalaran kedua"));
    assert!(credit.is_some());
    let credit_str = serde_json::to_string(&credit).expect("serialize credit succeeds");
    assert!(credit_str.contains("As-tsaqib"));

    let html =
        "<blockquote expandable>Catatan terlipat penting<cite>Dokumentasi</cite></blockquote>";
    let blocks_html = parse_markdown_to_rich_blocks(html);
    assert_eq!(blocks_html.len(), 1);
    let Some(RichBlock::ExpandableBlockQuotation {
        text: h_text,
        credit: h_credit,
    }) = blocks_html.first()
    else {
        panic!("expected HTML expandable blockquote");
    };
    assert!(serde_json::to_string(h_text)
        .expect("serialize h_text succeeds")
        .contains("Catatan terlipat penting"));
    assert!(serde_json::to_string(h_credit)
        .expect("serialize h_credit succeeds")
        .contains("Dokumentasi"));
}

#[test]
fn table_compact_and_caption_parse_correctly() {
    let text = "[table: Perbandingan Spesifikasi]\n| Model | Konteks |\n| :--- | :---: |\n| GPT-4o | 128k |\n| Claude | 200k |";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::Table {
        cells,
        is_compact,
        is_bordered,
        is_striped,
        caption,
        has_header,
        ..
    }) = blocks.first()
    else {
        panic!("expected rich table block");
    };
    assert!(*is_compact);
    assert!(!*is_bordered);
    assert!(!*is_striped);
    assert!(*has_header);
    assert_eq!(caption.as_deref(), Some("Perbandingan Spesifikasi"));
    assert_eq!(cells.len(), 3);
}

#[test]
fn tg_document_links_and_underline_parse_correctly() {
    let text = "Tautan: [Buka File](tg://document?id=doc_abc123) dan <u>garis bawah</u> serta ++format ins++";
    let parsed = parse_inline(text);
    let serialized = serde_json::to_string(&parsed).expect("serialize parsed succeeds");
    assert!(serialized.contains(r#""type":"url""#));
    assert!(serialized.contains("tg://document?id=doc_abc123"));
    assert!(serialized.contains("Buka File"));
    assert!(serialized.contains(r#""type":"underline""#));
    assert!(serialized.contains("garis bawah"));
    assert!(serialized.contains("format ins"));
}

#[test]
fn indonesian_and_case_insensitive_media_tags_parse_correctly() {
    let text = "[foto: Kucing Anggora](https://example.com/cat.jpg)\n\n[Foto : Kucing Lucu]  ( https://example.com/cat2.jpg ).\n\n[gambar: Pantai](https://example.com/beach.jpg)\n\n[dokumen: Laporan Keuangan](https://example.com/laporan.pdf)\n\n[file: Data Excel](https://example.com/data.xlsx)\n\n[musik: Suara Hujan](https://example.com/rain.mp3)\n\n[rekaman: Catatan Suara](https://example.com/voice.ogg)\n\n[lokasi: Monas, Jakarta](-6.175392, 106.827153)\n\n[kolase: Liburan](url1, url2)";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 9);
    assert!(matches!(blocks[0], RichBlock::Photo { .. }));
    assert!(matches!(blocks[1], RichBlock::Photo { .. }));
    assert!(matches!(blocks[2], RichBlock::Photo { .. }));
    assert!(matches!(blocks[3], RichBlock::Document { .. }));
    assert!(matches!(blocks[4], RichBlock::Document { .. }));
    assert!(matches!(blocks[5], RichBlock::Audio { .. }));
    assert!(matches!(blocks[6], RichBlock::VoiceNote { .. }));
    assert!(matches!(blocks[7], RichBlock::Map { .. }));
    assert!(matches!(blocks[8], RichBlock::Collage { .. }));
}

#[test]
fn embedded_media_blocks_in_paragraphs_are_isolated_and_parsed() {
    let text = "Ini fotonya: [photo: Kucing](https://example.com/cat.jpg) Kucing ini lucu.";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 3);
    assert!(matches!(blocks[0], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[1], RichBlock::Photo { .. }));
    assert!(matches!(blocks[2], RichBlock::Paragraph { .. }));

    let text_doc = "Silakan unduh dokumen [dokumen: Panduan](https://example.com/doc.pdf) yang telah kami siapkan.";
    let blocks_doc = parse_markdown_to_rich_blocks(text_doc);
    assert_eq!(blocks_doc.len(), 3);
    assert!(matches!(blocks_doc[0], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks_doc[1], RichBlock::Document { .. }));
    assert!(matches!(blocks_doc[2], RichBlock::Paragraph { .. }));
}

#[test]
fn html_tags_convert_to_rich_formatting() {
    let input = "Teks <b>tebal</b> dan <strong>kuat</strong> serta <i>miring</i> dan <code>kode()</code> serta <a href=\"https://example.com\">Tautan</a>";
    let value = parse_inline(input);
    let serialized = serde_json::to_string(&value).expect("serialize value succeeds");
    assert!(serialized.contains(r#""type":"bold""#));
    assert!(serialized.contains("tebal"));
    assert!(serialized.contains("kuat"));
    assert!(serialized.contains(r#""type":"italic""#));
    assert!(serialized.contains("miring"));
    assert!(serialized.contains(r#""type":"code""#));
    assert!(serialized.contains("kode()"));
    assert!(serialized.contains(r#""type":"url""#));
    assert!(serialized.contains("https://example.com"));
}

#[test]
fn github_alert_callouts_parse_correctly() {
    let text = "> [!NOTE]\n> Ini catatan penting sistem.";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 1);
    let serialized = serde_json::to_string(&blocks[0]).expect("serialize callout succeeds");
    assert!(serialized.contains("Catatan:"));
    assert!(serialized.contains("Ini catatan penting sistem."));
}

#[test]
fn headings_without_space_parse_correctly() {
    let text = "###Fitur Baru\n\nPenjelasan fitur.";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        blocks[0],
        RichBlock::SectionHeading { level: 3, .. }
    ));
}

#[test]
fn leaked_thinking_and_tool_calls_are_stripped() {
    let text = "<think>\nInternal secret reasoning\n</think>\n<tool_call>\n{\"name\": \"search\"}\n</tool_call>\nHalo! Ada yang bisa dibantu?";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 1);
    let serialized = serde_json::to_string(&blocks[0]).expect("serialize stripped block succeeds");
    assert!(!serialized.contains("Internal secret reasoning"));
    assert!(!serialized.contains("tool_call"));
    assert!(serialized.contains("Halo! Ada yang bisa dibantu?"));
}

#[test]
fn streaming_markdown_never_leaks_unclosed_thinking_or_provisional_artifacts() {
    for text in [
        "<think>\ntunggu sebentar, saya sedang mencari - referensi",
        "<thought>\ntunggu sebentar, saya sedang - mencari",
        "<reasoning>\nsedang memikirkan - langkah",
        "<think>proses awal</think>\n<thought>proses kedua - lanjutan",
        "[thinking]\nproses bracket - pemikiran",
        "[think]\nproses bracket - singkat",
        "<",
        "<th",
        "<think",
        "<thought",
        "[",
        "[th",
        "[think",
        "[thinking",
    ] {
        let blocks = parse_streaming_markdown_to_rich_blocks(text);
        assert!(
            blocks.is_empty(),
            "expected empty blocks for thinking draft '{text}', but got {blocks:?}"
        );
    }

    // Ensure normal markdown link with [thinking] text is not wiped
    let normal_link = "[thinking](https://example.com) adalah link normal";
    let blocks = parse_streaming_markdown_to_rich_blocks(normal_link);
    assert!(!blocks.is_empty(), "expected markdown link to be preserved");
}

#[test]
fn streaming_video_urls_do_not_produce_raw_video_blocks() {
    let text = "[video: Belajar Rust](https://www.youtube.com/watch?v=5C_HPTJg5ek)\n\n![Tutorial](https://youtu.be/abc12345)";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 2);
    // Should parse as Paragraphs with styled links so Telegram link preview works without API 400 rejection
    assert!(matches!(blocks[0], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[1], RichBlock::Paragraph { .. }));
    let s0 = serde_json::to_string(&blocks[0]).expect("serialize block 0 succeeds");
    let s1 = serde_json::to_string(&blocks[1]).expect("serialize block 1 succeeds");
    assert!(s0.contains("Belajar Rust") && s0.contains("youtube.com"));
    assert!(s1.contains("Tutorial") && s1.contains("youtu.be"));
}

#[test]
fn direct_video_files_produce_native_video_blocks() {
    let text = "[video: Animasi Robot](https://example.com/demo.mp4)\n\n![Clip](https://example.com/sample.webm)";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 2);
    assert!(matches!(blocks[0], RichBlock::Video { .. }));
    assert!(matches!(blocks[1], RichBlock::Video { .. }));
}

#[test]
fn telegram_html_media_tags_parse_into_rich_blocks() {
    let text = "<tg-photo src=\"https://example.com/cat.jpg\" caption=\"Kucing Manis\"/>\n\n<tg-audio src=\"https://example.com/audio.mp3\" caption=\"Lagu Pengantar\"/>\n\n<img src=\"https://example.com/pic.png\" alt=\"Foto Profil\">";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 3);
    assert!(matches!(blocks[0], RichBlock::Photo { .. }));
    assert!(matches!(blocks[1], RichBlock::Audio { .. }));
    assert!(matches!(blocks[2], RichBlock::Photo { .. }));
    let cap = blocks[0].caption_text().expect("caption present");
    assert_eq!(cap, "Kucing Manis");
}

#[test]
fn multi_line_tg_collage_and_slideshow_parse_correctly() {
    let collage_html = r#"<tg-collage caption="Koleksi Logo">
<tg-photo src="https://example.com/logo1.png"/>
<tg-photo src="https://example.com/logo2.png"/>
</tg-collage>"#;
    let blocks = parse_markdown_to_rich_blocks(collage_html);
    assert_eq!(blocks.len(), 1);
    let RichBlock::Collage {
        blocks: items,
        caption: _,
    } = &blocks[0]
    else {
        panic!("expected collage block");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(blocks[0].caption_text().as_deref(), Some("Koleksi Logo"));

    let slideshow_html = r#"<tg-slideshow caption="Alur Slide">
<tg-photo src="https://example.com/s1.jpg"/>
<tg-photo src="https://example.com/s2.jpg"/>
</tg-slideshow>"#;
    let s_blocks = parse_markdown_to_rich_blocks(slideshow_html);
    assert_eq!(s_blocks.len(), 1);
    assert!(matches!(s_blocks[0], RichBlock::Slideshow { .. }));
}

#[test]
fn multiple_consecutive_photos_parse_into_separate_rich_blocks() {
    let text = "Berikut logonya:\n\n[photo: Logo Rust](https://example.com/rust.png)\n[photo: Logo Go](https://example.com/go.png)\n[photo: Logo Python](https://example.com/py.png)";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 4);
    assert!(matches!(blocks[0], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[1], RichBlock::Photo { .. }));
    assert!(matches!(blocks[2], RichBlock::Photo { .. }));
    assert!(matches!(blocks[3], RichBlock::Photo { .. }));
}

#[test]
fn unsupported_image_formats_and_streaming_audio_produce_emoji_links() {
    let text = "[photo: Vektor SVG](https://example.com/logo.svg)\n\n![Audio](https://open.spotify.com/track/12345)\n\n<tg-photo src=\"https://example.com/art.bmp\" caption=\"Gambar Bitmap\"/>\n\n![](https://example.com/vector.svg)";
    let blocks = parse_markdown_to_rich_blocks(text);
    assert_eq!(blocks.len(), 4);
    assert!(matches!(blocks[0], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[1], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[2], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[3], RichBlock::Paragraph { .. }));
    let s0 = serde_json::to_string(&blocks[0]).expect("serialize block 0 succeeds");
    let s1 = serde_json::to_string(&blocks[1]).expect("serialize block 1 succeeds");
    let s2 = serde_json::to_string(&blocks[2]).expect("serialize block 2 succeeds");
    let s3 = serde_json::to_string(&blocks[3]).expect("serialize block 3 succeeds");
    assert!(s0.contains("🖼️") && s0.contains("logo.svg") && s0.contains("Vektor SVG"));
    assert!(s1.contains("🎵") && s1.contains("spotify.com") && s1.contains("Audio"));
    assert!(s2.contains("🖼️") && s2.contains("art.bmp") && s2.contains("Gambar Bitmap"));
    assert!(s3.contains("🖼️") && s3.contains("vector.svg") && s3.contains("Lihat Foto"));
}

#[test]
fn math_blocks_and_inline_math_are_sanitized_for_cross_platform_rendering() {
    let md = r#"2. Teorema Pythagoras
$$c = \sqrt{a^2 + b^2} = \sqrt{6^2 + 8^2}$$
$$= \sqrt{36 + 64} = \sqrt{100} = 10\text{cm}$$

Contoh inline: $44\text{cm}$ dan $7,5\text{hari}$."#;

    let blocks = parse_markdown_to_rich_blocks(md);
    let math_blocks: Vec<_> = blocks
        .iter()
        .filter_map(|b| match b {
            RichBlock::MathematicalExpression { expression } => Some(expression.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(math_blocks.len(), 2);
    assert_eq!(math_blocks[0], r"c = \sqrt{a^2 + b^2} = \sqrt{6^2 + 8^2}");
    assert_eq!(
        math_blocks[1],
        r"= \sqrt{36 + 64} = \sqrt{100} = 10\ \mathrm{cm}"
    );

    // Verify inline math serialization inside paragraph
    let paragraph = blocks
        .iter()
        .find(|b| matches!(b, RichBlock::Paragraph { .. }))
        .expect("paragraph block present");
    let serialized = serde_json::to_string(paragraph).expect("serialize paragraph succeeds");
    assert!(serialized.contains(r"44\\ \\mathrm{cm}"));
    assert!(serialized.contains(r"7.5\\ \\mathrm{hari}"));
}

#[test]
fn multiline_fenced_math_emits_individual_rich_blocks_per_line() {
    let md = "$$\nc = \\sqrt{a^2 + b^2}\n= \\sqrt{36 + 64}\n= 10\\text{cm}\n$$";
    let blocks = parse_markdown_to_rich_blocks(md);
    let math_blocks: Vec<_> = blocks
        .iter()
        .filter_map(|b| match b {
            RichBlock::MathematicalExpression { expression } => Some(expression.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(math_blocks.len(), 3);
    assert_eq!(math_blocks[0], r"c = \sqrt{a^2 + b^2}");
    assert_eq!(math_blocks[1], r"= \sqrt{36 + 64}");
    assert_eq!(math_blocks[2], r"= 10\ \mathrm{cm}");
}

#[test]
fn rtl_markdown_table_with_hindi_numerals_defaults_to_right_alignment_and_sets_is_rtl() {
    let md = "| الرقم | الاسم |\n| --- | --- |\n| ١ | أحمد |\n| ٢ | فاطمة |";
    let message = build_full_rich_message(md, None);
    assert_eq!(message.is_rtl, Some(true));

    let Some(RichBlock::Table { cells, .. }) = message.blocks.first() else {
        panic!("expected table block");
    };

    // All cells in RTL table with unspecified separator default to "right"
    assert_eq!(cells[0][0].align.as_deref(), Some("right"));
    assert_eq!(cells[0][1].align.as_deref(), Some("right"));
    assert_eq!(cells[1][0].align.as_deref(), Some("right"));
    assert_eq!(cells[1][1].align.as_deref(), Some("right"));
}

#[test]
fn rtl_table_honors_explicit_column_alignment() {
    let md = "| الرقم | الاسم | النتيجة |\n| :--- | :---: | ---: |\n| ١ | أحمد | ممتاز |";
    let blocks = parse_markdown_to_rich_blocks(md);
    let Some(RichBlock::Table { cells, .. }) = blocks.first() else {
        panic!("expected table block");
    };

    assert_eq!(cells[0][0].align.as_deref(), Some("left"));
    assert_eq!(cells[0][1].align.as_deref(), Some("center"));
    assert_eq!(cells[0][2].align.as_deref(), Some("right"));

    assert_eq!(cells[1][0].align.as_deref(), Some("left"));
    assert_eq!(cells[1][1].align.as_deref(), Some("center"));
    assert_eq!(cells[1][2].align.as_deref(), Some("right"));
}

#[test]
fn unicode_box_table_with_rtl_header_defaults_column_to_right_alignment() {
    let input =
        "┌──────┬──────┐\n│ الرقم │ Score│\n├──────┼──────┤\n│ 123  │ 98   │\n└──────┴──────┘";
    let blocks = parse_markdown_to_rich_blocks(input);
    let Some(RichBlock::Table { cells, .. }) = blocks.first() else {
        panic!("expected table block");
    };

    // Col 0 has RTL header "الرقم", so even though cell is ASCII "123", it defaults to "right"
    assert_eq!(cells[0][0].align.as_deref(), Some("right"));
    assert_eq!(cells[1][0].align.as_deref(), Some("right"));

    // Col 1 is pure Latin "Score" / "98", so it remains "left"
    assert_eq!(cells[0][1].align.as_deref(), Some("left"));
    assert_eq!(cells[1][1].align.as_deref(), Some("left"));
}

#[test]
fn test_split_table_row_cells_preserves_math_pipes() {
    let row = r"| $|v|_p$ | Norma- $p$ | $\left( \sum |v_i|^p \right)^{1/p}$ |";
    let cells = split_table_row_cells(row, false);
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0], r"$|v|_p$");
    assert_eq!(cells[1], r"Norma- $p$");
    assert_eq!(cells[2], r"$\left( \sum |v_i|^p \right)^{1/p}$");
}

#[test]
fn test_norm_table_with_math_pipes_parses_three_columns() {
    let md = "| Simbol | Nama | Definisi |\n| :---: | :--- | :--- |\n| $|v|_p$ | Norma- $p$ | $\\left( \\sum |v_i|^p \\right)^{1/p}$ |\n";
    let blocks = parse_markdown_to_rich_blocks(md);
    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::Table { cells, .. }) = blocks.first() else {
        panic!("expected table block");
    };
    assert_eq!(cells.len(), 2); // 1 header row, 1 data row
    assert_eq!(cells[0].len(), 3); // 3 header columns
    assert_eq!(cells[1].len(), 3); // 3 data columns!

    // Check third cell of second row contains single mathematical_expression
    let json = serde_json::to_string(&cells[1][2]).expect("cell serializes");
    assert!(json.contains("mathematical_expression"));
    assert!(json.contains(r"\\left( \\sum |v_i|^p \\right)^{1/p}"));
}

#[test]
fn test_standalone_therefore_and_because_render_as_unicode() {
    let md = "| Simbol | Arti / Nama | Penjelasan |\n| :---: | :--- | :--- |\n| $\\therefore$ | Oleh karena itu | Kesimpulan logis |\n| $\\because$ | Karena | Alasan/Premis |\n| $\\implies$ | Implikasi | Jika... maka... |\n| $\\impliedby$ | Implikasi balik | ...jika... |\n";
    let blocks = parse_markdown_to_rich_blocks(md);
    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::Table { cells, .. }) = blocks.first() else {
        panic!("expected table block");
    };
    assert_eq!(cells.len(), 5);

    // Row 1: \therefore -> text "∴" (must be plain string, not unsupported plain_text entity)
    let cell_therefore = serde_json::to_string(&cells[1][0]).expect("cell serializes");
    assert!(cell_therefore.contains(r#""text":"∴""#));
    assert!(!cell_therefore.contains(r#""type":"plain_text""#));

    // Row 2: \because -> text "∵" (must be plain string, not unsupported plain_text entity)
    let cell_because = serde_json::to_string(&cells[2][0]).expect("cell serializes");
    assert!(cell_because.contains(r#""text":"∵""#));
    assert!(!cell_because.contains(r#""type":"plain_text""#));

    // Row 3: \implies -> mathematical_expression \implies
    let cell_implies = serde_json::to_string(&cells[3][0]).expect("cell serializes");
    assert!(cell_implies.contains(r#""expression":"\\implies""#));

    // Row 4: \impliedby -> normalized to \Longleftarrow for SwiftMath
    let cell_impliedby = serde_json::to_string(&cells[4][0]).expect("cell serializes");
    assert!(cell_impliedby.contains(r#""expression":"\\Longleftarrow""#));
}

#[test]
fn test_reproduce_math_logic_table_no_unsupported_plain_text() {
    let md = r#"### 4. Logika Matematika & Pembuktian

| Simbol | Nama / Arti | Makna / Contoh |
| :---: | :--- | :--- |
| $\neg$ / $\sim$ | Negasi / Ingkaran | Menyangkal pernyataan ("bukan" / $\neg P$) |
| $\land$ | Konjungsi | Logika "dan" ($P \land Q$) |
| $\lor$ | Disjungsi | Logika "atau" ($P \lor Q$) |
| $\oplus$ | *Exclusive OR* (XOR) | Benar jika salah satu benar, tapi tidak keduanya |
| $\implies$ / $\to$ | Implikasi | "Jika $P$ maka $Q$" ($P \implies Q$) |
| $\iff$ / $\leftrightarrow$ | Biimplikasi | "Jika dan hanya jika" ($P \iff Q$) |
| $\forall$ | Kuantor Universal | "Untuk setiap / untuk semua" ($\forall x \in \mathbb{R}$) |
| $\exists$ | Kuantor Eksistensial | "Ada / terdapat setidaknya satu" ($\exists x$) |
| $\nexists$ | Negasi Eksistensial | "Tidak ada" |
| $\exists!$ | Keunikan | "Ada tepat satu" |
| $\therefore$ | Maka / Oleh karena itu | Penarikan kesimpulan (*Therefore*) |
| $\because$ | Karena | Memberikan alasan (*Because*) |
| $\blacksquare$ / Q.E.D. | Akhir pembuktian | *Quod Erat Demonstrandum* (telah terbukti) |
"#;
    let blocks = parse_markdown_to_rich_blocks(md);
    let msg = crate::bot::models::InputRichMessage::new(blocks);
    let val_res = msg.validate();
    assert!(val_res.is_ok(), "Validation failed: {:?}", val_res);

    let json_str = serde_json::to_string(&msg).expect("serialize rich message");
    assert!(
        !json_str.contains(r#""type":"plain_text""#),
        "Telegram Bot API rejects 'plain_text' as an unsupported rich text type"
    );
    assert!(
        !json_str.contains(r#""plain_text""#),
        "No plain_text discriminator should ever appear in rich message entities"
    );
}

#[test]
fn indonesian_nahwu_lesson_preserves_ltr_canvas_and_correct_table_order() {
    let md = r#"### 4. Contoh Analisis Kalimat Sederhana

Mari kita bedah kalimat ini:
> **كَتَبَ التِّلْمِيْذُ الدَّرْسَ** (*Kataba at-tilmiidzu ad-darsa*)
Artinya: *Murid itu telah menulis pelajaran.*

1. **كَتَبَ** (*Kataba*): Fi'il Madhi (Kata kerja lampau).
2. **التِّلْمِيْذُ** (*At-tilmiidzu*): Fa'il (Pelaku), wajib berstatus *Rofa'*.
3. **الدَّرْسَ** (*Ad-darsa*): Maf'ul Bih (Objek), wajib berstatus *Nashab*.

| Nama I'rab | Tanda Asli (Harakat) | Biasanya Dipakai Untuk | Contoh |
| :--- | :---: | :--- | ---: |
| **Rofa'** | Dhammah (ـُ) | Subjek / Pelaku (*Fa'il*) | جَاءَ رَجُلٌ |
| **Nashab** | Fathah (ـَ) | Objek penderita (*Maf'ul Bih*) | رَأَيْتُ رَجُلاً |

### Ringkasan untuk Pemula:
1. Kenali dulu apakah suatu kata itu Benda (Isim), Kerja (Fi'il), atau Huruf.
2. Perhatikan awal kalimatnya: dimulai Isim atau Fi'il.
"#;
    let msg = build_full_rich_message(md, None);
    // The message is predominantly Indonesian, so is_rtl MUST be None
    assert_eq!(
        msg.is_rtl, None,
        "Mixed Indonesian lesson must not trigger global is_rtl"
    );

    // Verify the table block
    let table_block = msg
        .blocks
        .iter()
        .find(|b| matches!(b, RichBlock::Table { .. }))
        .expect("must contain a table block");

    let RichBlock::Table { cells, .. } = table_block else {
        panic!("expected table");
    };

    // Table column 0 must remain "Nama I'rab" (LTR column order preserved)
    let col0_header_text = &cells[0][0].text;
    assert!(
        serde_json::to_string(col0_header_text)
            .expect("serialize")
            .contains("Nama I'rab"),
        "Column 0 must remain 'Nama I'rab' on the left"
    );

    // Column 3 must be "Contoh" with right alignment
    let col3_header_text = &cells[0][3].text;
    assert!(
        serde_json::to_string(col3_header_text)
            .expect("serialize")
            .contains("Contoh"),
        "Column 3 must be 'Contoh'"
    );
    assert_eq!(cells[0][3].align.as_deref(), Some("right"));
    assert_eq!(cells[1][3].align.as_deref(), Some("right"));

    // Verify lists
    let list_blocks: Vec<_> = msg
        .blocks
        .iter()
        .filter(|b| matches!(b, RichBlock::List { .. }))
        .collect();
    assert_eq!(list_blocks.len(), 2, "Must contain 2 lists");
}

#[test]
fn arabic_table_inside_ltr_message_is_reversed_with_eastern_arabic_digits() {
    let md = r#"Berikut adalah daftar santri teladan:

| الرقم | الاسم |
| :---: | :---: |
| 1 | أحمد |
| 2 | فاطمة |

Semoga bermanfaat untuk kita semua.
"#;
    let msg = build_full_rich_message(md, None);
    // Surrounding text is Indonesian -> is_rtl is None
    assert_eq!(msg.is_rtl, None);

    let table_block = msg
        .blocks
        .iter()
        .find(|b| matches!(b, RichBlock::Table { .. }))
        .expect("must contain a table block");

    let RichBlock::Table { cells, .. } = table_block else {
        panic!("expected table");
    };

    // Because header is pure Arabic (| الرقم | الاسم |) in an LTR message,
    // columns are reversed so that Column 0 (الرقم) appears visually on the right
    let col0_text = serde_json::to_string(&cells[0][0].text).expect("serialize");
    let col1_text = serde_json::to_string(&cells[0][1].text).expect("serialize");
    assert!(
        col0_text.contains("الاسم"),
        "Reversed: 'الاسم' should be at index 0"
    );
    assert!(
        col1_text.contains("الرقم"),
        "Reversed: 'الرقم' should be at index 1 (right edge)"
    );

    // Digits in the number column are converted to Eastern Arabic numerals
    let row1_num_cell = serde_json::to_string(&cells[1][1].text).expect("serialize");
    assert!(
        row1_num_cell.contains('١'),
        "Row 1 number should be Eastern Arabic '١'"
    );

    let row2_num_cell = serde_json::to_string(&cells[2][1].text).expect("serialize");
    assert!(
        row2_num_cell.contains('٢'),
        "Row 2 number should be Eastern Arabic '٢'"
    );
}

#[test]
fn eastern_arabic_ordered_list_parses_value_correctly() {
    let md = r#"١. كتب الطالب الدرس
٢. قرأ زيد الكتاب
٣. جلس المعلم في الفصل
"#;
    let blocks = parse_markdown_to_rich_blocks(md);
    assert_eq!(blocks.len(), 1);
    let RichBlock::List { items } = &blocks[0] else {
        panic!("expected list");
    };
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].value, Some(1));
    assert_eq!(items[1].value, Some(2));
    assert_eq!(items[2].value, Some(3));
}

#[test]
fn test_arabic_in_display_math_becomes_block_quotation() {
    let md = "Jika ditinjau:\n\n$$\\text{لَا تَقْنَطُوا مِنْ رَحْمَةِ اللَّهِ}$$\n\n* **لَا (Lā)**";
    let blocks = parse_markdown_to_rich_blocks(md);
    assert_eq!(blocks.len(), 3);
    assert!(matches!(blocks[0], RichBlock::Paragraph { .. }));
    let RichBlock::BlockQuotation {
        blocks: quote_blocks,
    } = &blocks[1]
    else {
        panic!("expected BlockQuotation, got {:?}", blocks[1]);
    };
    let quote_json = serde_json::to_string(&quote_blocks[0]).expect("serialize");
    assert!(quote_json.contains("لَا تَقْنَطُوا مِنْ رَحْمَةِ اللَّهِ"));
    assert!(!quote_json.contains(r"\text"));
    assert!(!quote_json.contains(r"\mathrm"));

    // Verify that NO MathematicalExpression contains RTL
    let has_math_arabic = blocks.iter().any(|b| match b {
        RichBlock::MathematicalExpression { expression } => {
            crate::parser::rtl::has_rtl_characters(expression)
        }
        _ => false,
    });
    assert!(!has_math_arabic);
}

#[test]
fn test_arabic_in_bracket_math_becomes_block_quotation() {
    let md = r#"Kutipan:
\[ \text{إِنَّ مَعَ الْعُسْرِ يُسْرًا} \]
Penjelasan berikutnya."#;
    let blocks = parse_markdown_to_rich_blocks(md);
    assert_eq!(blocks.len(), 3);
    let RichBlock::BlockQuotation {
        blocks: quote_blocks,
    } = &blocks[1]
    else {
        panic!("expected BlockQuotation, got {:?}", blocks[1]);
    };
    let quote_json = serde_json::to_string(&quote_blocks[0]).expect("serialize");
    assert!(quote_json.contains("إِنَّ مَعَ الْعُسْرِ يُسْرًا"));
    assert!(!quote_json.contains(r"\text"));
}

#[test]
fn test_arabic_in_inline_math_becomes_native_inline_text() {
    let md = "Perhatikan kata $\\text{لَا}$ di dalam kalimat.";
    let blocks = parse_markdown_to_rich_blocks(md);
    assert_eq!(blocks.len(), 1);
    let RichBlock::Paragraph { text } = &blocks[0] else {
        panic!("expected Paragraph");
    };
    let para_json = serde_json::to_string(text).expect("serialize");
    assert!(para_json.contains("لَا"));
    assert!(!para_json.contains("mathematical_expression"));
}

#[test]
fn test_genuine_math_formulas_still_produce_mathematical_expression() {
    let md = "$$c = \\sqrt{a^2 + b^2}$$\n\nInline: $E = mc^2$";
    let blocks = parse_markdown_to_rich_blocks(md);
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        blocks[0],
        RichBlock::MathematicalExpression { .. }
    ));
    let RichBlock::Paragraph { text } = &blocks[1] else {
        panic!("expected Paragraph");
    };
    let para_json = serde_json::to_string(text).expect("serialize");
    assert!(para_json.contains("mathematical_expression"));
}

#[test]
fn test_turn_34_exact_nahwu_snippet_parses_without_rtl_in_math() {
    let md = r#"### **Sentuhan Nahwu & Kebahasaan**

Jika ditinjau dari kaidah tata bahasa Arab (*nahwu*), penggalan kalimat tersebut mengandung uslub larangan (*an-nahyu*):

$$\text{لَا تَقْنَطُوا مِنْ رَحْمَةِ اللَّهِ}$$

* **لَا (Lā)**: Disebut **لَا النَّاهِيَةُ** (*Lā an-Nāhiyah*), yaitu huruf yang bermakna larangan ("janganlah") dan bersifat menjazamkan kata kerja mudhari' (*tajzumu al-fi'l al-mudhāri'*).
* **تَقْنَطُوا (Taqnathū)**: Adalah **فِعْلٌ مُضَارِعٌ مَجْزُومٌ** (*fi'il mudhāri' majzūm*) dengan tanda jazam **حَذْفُ النُّونِ** (dibuangnya huruf nun) karena termasuk ke dalam kelompok **الْأَفْعَالُ الْخَمْسَةُ** (*al-af'āl al-khamsah* — bentuk asalnya sebelum kemasukan *lā* adalah *taqnathūna* / تَقْنَطُونَ).
  * Huruf **Wawu** (و) di dalamnya berposisi sebagai dhamir fail (*fā'il* / subjek).
* **مِنْ (Min)**: Huruf jar (*harf jarr*).
* **رَحْمَةِ (Rahmati)**: Isim majrur tanda kasrah, sekaligus berposisi sebagai **mudhaf** (مُضَاف).
* **اللَّهِ (Allāh)**: Lafaz jalalah sebagai **mudhaf ilaih** (مُضَاف إِلَيْهِ) yang majrur dengan kasrah di akhirnya.
"#;
    let rich = build_full_rich_message(md, None);
    // Ensure no block is a MathematicalExpression with Arabic
    for b in &rich.blocks {
        if let RichBlock::MathematicalExpression { expression } = b {
            assert!(
                !crate::parser::rtl::has_rtl_characters(expression),
                "MathematicalExpression should not contain Arabic: {expression}"
            );
        }
    }
    // Ensure the Arabic verse appears in a BlockQuotation
    let has_quote = rich.blocks.iter().any(|b| {
        if let RichBlock::BlockQuotation { blocks } = b {
            let s = serde_json::to_string(blocks).expect("serialize");
            s.contains("لَا تَقْنَطُوا مِنْ رَحْمَةِ اللَّهِ")
        } else {
            false
        }
    });
    assert!(has_quote, "Arabic phrase should be inside a BlockQuotation");
}

#[test]
fn test_mixed_arabic_indonesian_list_items_get_lrm_prefix() {
    let md = r#"
Berikut adalah uraian I'rab:

> **وَلْيَكْتُبْ بَيْنَكُمْ كَاتِبٌ بِالْعَدْلِ ۚ**

* **يَا (Yā)**: *Harf nidā'* (huruf panggilan) mabni di atas sukun.
* **أَيُّ (Ayyu)**: *Munāda* mabni di atas dhammah.
* Fa (فَ): Rābiṭah li-jawāb asy-syarṭ (penghubung jawaban syarat).

**اللَّهِ**: Lafaz jalalah sebagai mudhaf ilaih.
"#;
    let blocks = parse_markdown_to_rich_blocks(md);

    // 1. Pure Arabic quote box should NOT have LRM prefix
    let quote = blocks
        .iter()
        .find(|b| matches!(b, RichBlock::BlockQuotation { .. }))
        .expect("find quote block");
    let quote_json = serde_json::to_string(quote).expect("serialize quote");
    assert!(
        !quote_json.contains('\u{200E}'),
        "Pure Arabic quote should NOT contain LRM"
    );

    // 2. List items
    let list_block = blocks
        .iter()
        .find(|b| matches!(b, RichBlock::List { .. }))
        .expect("find list block");
    let RichBlock::List { items } = list_block else {
        panic!("expected list");
    };
    assert_eq!(items.len(), 3);

    // Item 0 starts with Arabic `يَا` and has Indonesian text -> MUST have LRM `\u{200E}`
    let item0_json = serde_json::to_string(&items[0]).expect("serialize item 0");
    assert!(
        item0_json.contains('\u{200E}'),
        "Mixed item 0 starting with Arabic must have LRM: {item0_json}"
    );

    // Item 1 starts with Arabic `أَيُّ` and has Indonesian text -> MUST have LRM `\u{200E}`
    let item1_json = serde_json::to_string(&items[1]).expect("serialize item 1");
    assert!(
        item1_json.contains('\u{200E}'),
        "Mixed item 1 starting with Arabic must have LRM: {item1_json}"
    );

    // Item 2 starts with Latin `Fa` -> MUST NOT have LRM `\u{200E}`
    let item2_json = serde_json::to_string(&items[2]).expect("serialize item 2");
    assert!(
        !item2_json.contains('\u{200E}'),
        "Item 2 starting with Latin should NOT have LRM: {item2_json}"
    );

    // 3. Mixed paragraph starting with Arabic **اللَّهِ**: Lafaz jalalah... -> MUST have LRM
    let mixed_para = blocks
        .iter()
        .find(|b| {
            if let RichBlock::Paragraph { text } = b {
                serde_json::to_string(text)
                    .unwrap_or_default()
                    .contains("mudhaf ilaih")
            } else {
                false
            }
        })
        .expect("find mixed paragraph");
    let para_json = serde_json::to_string(mixed_para).expect("serialize mixed para");
    assert!(
        para_json.contains('\u{200E}'),
        "Mixed paragraph starting with Arabic must have LRM: {para_json}"
    );
}

// =========================================================================
// Milestone M2: Rich Tag HTML Parser & Media Resolution Tests
// =========================================================================

#[test]
fn test_parse_rich_html_single_img_with_tg_scheme() {
    let html = r#"<img src="tg://photo?id=pic_summit" alt="Puncak Rinjani"/>"#;
    let res = parse_rich_html(html);
    assert!(res.is_ok(), "Expected Ok, got: {res:?}");
    let (blocks, media) = res.expect("valid result");

    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::Photo { photo, caption }) = blocks.first() else {
        panic!("Expected RichBlock::Photo, got: {:?}", blocks.first());
    };
    assert_eq!(photo["type"], "photo");
    assert_eq!(photo["media"], "tg://photo?id=pic_summit");
    assert_eq!(
        caption
            .as_ref()
            .map(|c| serde_json::to_string(&c.text).unwrap_or_default()),
        Some("\"Puncak Rinjani\"".to_string())
    );

    assert_eq!(media.len(), 1);
    assert_eq!(media[0].id, "pic_summit");
    assert_eq!(media[0].media.media_url(), "tg://photo?id=pic_summit");
    assert_eq!(media[0].media.caption_text(), Some("Puncak Rinjani"));
}

#[test]
fn test_parse_rich_html_single_img_with_direct_url() {
    let html = r#"<img src="https://example.com/rinjani.jpg" caption="Puncak Matahari Terbit"/>"#;
    let (blocks, media) = parse_rich_html(html).expect("valid direct url img");

    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::Photo { photo, caption }) = blocks.first() else {
        panic!("Expected RichBlock::Photo");
    };
    assert_eq!(photo["type"], "photo");
    assert_eq!(photo["media"], "https://example.com/rinjani.jpg");
    assert!(caption.is_some());

    assert_eq!(media.len(), 1);
    assert_eq!(media[0].id, "photo_1");
    assert_eq!(
        media[0].media.media_url(),
        "https://example.com/rinjani.jpg"
    );
    assert_eq!(
        media[0].media.caption_text(),
        Some("Puncak Matahari Terbit")
    );
}

#[test]
fn test_parse_rich_html_audio_tag_attributes() {
    let html = r#"<audio src="tg://audio?id=aud1" title="Angin Sembalun" performer="Lombok Sounds" caption="Suara Alam"/>"#;
    let (blocks, media) = parse_rich_html(html).expect("valid audio tag");

    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::Audio { audio, caption }) = blocks.first() else {
        panic!("Expected RichBlock::Audio");
    };
    assert_eq!(audio["type"], "audio");
    assert_eq!(audio["media"], "tg://audio?id=aud1");
    assert_eq!(audio["title"], "Angin Sembalun");
    assert_eq!(audio["performer"], "Lombok Sounds");
    assert_eq!(
        caption
            .as_ref()
            .map(|c| serde_json::to_string(&c.text).unwrap_or_default()),
        Some("\"Suara Alam\"".to_string())
    );

    assert_eq!(media.len(), 1);
    assert_eq!(media[0].id, "aud1");
    assert_eq!(media[0].media.media_url(), "tg://audio?id=aud1");
    if let InputMedia::Audio {
        title,
        performer,
        caption,
        ..
    } = &media[0].media
    {
        assert_eq!(title.as_deref(), Some("Angin Sembalun"));
        assert_eq!(performer.as_deref(), Some("Lombok Sounds"));
        assert_eq!(caption.as_deref(), Some("Suara Alam"));
    } else {
        panic!("Expected InputMedia::Audio");
    }
}

#[test]
fn test_parse_rich_html_collage_container() {
    let html = r#"<tg-collage caption="Album Kawah"><img src="tg://photo?id=p1" alt="Danau"/><img src="tg://photo?id=p2" alt="Puncak"/></tg-collage>"#;
    let (blocks, media) = parse_rich_html(html).expect("valid collage");

    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::Collage {
        blocks: child_blocks,
        caption,
    }) = blocks.first()
    else {
        panic!("Expected RichBlock::Collage");
    };
    assert_eq!(child_blocks.len(), 2);
    assert_eq!(child_blocks[0]["type"], "photo");
    assert_eq!(child_blocks[0]["photo"]["media"], "tg://photo?id=p1");
    assert_eq!(child_blocks[0]["photo"]["caption"], "Danau");
    assert_eq!(child_blocks[1]["photo"]["media"], "tg://photo?id=p2");
    assert_eq!(child_blocks[1]["photo"]["caption"], "Puncak");
    assert_eq!(
        caption
            .as_ref()
            .map(|c| serde_json::to_string(&c.text).unwrap_or_default()),
        Some("\"Album Kawah\"".to_string())
    );

    assert_eq!(media.len(), 2);
    assert_eq!(media[0].id, "p1");
    assert_eq!(media[0].media.caption_text(), Some("Danau"));
    assert_eq!(media[1].id, "p2");
    assert_eq!(media[1].media.caption_text(), Some("Puncak"));
}

#[test]
fn test_parse_rich_html_slideshow_container() {
    let html = r#"<tg-slideshow caption="Slideshow Pendakian"><img src="tg://photo?id=s1"/><img src="tg://photo?id=s2"/><img src="tg://photo?id=s3"/></tg-slideshow>"#;
    let (blocks, media) = parse_rich_html(html).expect("valid slideshow");

    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::Slideshow {
        blocks: child_blocks,
        caption,
    }) = blocks.first()
    else {
        panic!("Expected RichBlock::Slideshow");
    };
    assert_eq!(child_blocks.len(), 3);
    assert!(caption.is_some());
    assert_eq!(media.len(), 3);
    assert_eq!(media[0].id, "s1");
    assert_eq!(media[1].id, "s2");
    assert_eq!(media[2].id, "s3");
}

#[test]
fn test_parse_rich_html_tg_map_valid_coordinates_and_zoom() {
    let html =
        r#"<tg-map lat="-8.4113" lon="116.4573" zoom="13" title="Puncak Rinjani 3.726 mdpl"/>"#;
    let (blocks, media) = parse_rich_html(html).expect("valid map tag");

    assert_eq!(blocks.len(), 1);
    let Some(RichBlock::Map { location, zoom, .. }) = blocks.first() else {
        panic!("Expected RichBlock::Map");
    };
    assert_eq!(location.latitude, -8.4113);
    assert_eq!(location.longitude, 116.4573);
    assert_eq!(*zoom, Some(13));
    assert!(media.is_empty(), "Map produces no media upload items");
}

#[test]
fn test_parse_rich_html_tg_map_rejects_out_of_bounds_and_non_finite() {
    // Latitude out of bounds [-90, 90]
    assert!(parse_rich_html(r#"<tg-map lat="91.0" lon="0.0"/>"#).is_err());
    assert!(parse_rich_html(r#"<tg-map lat="-91.0" lon="0.0"/>"#).is_err());

    // Longitude out of bounds [-180, 180]
    assert!(parse_rich_html(r#"<tg-map lat="0.0" lon="181.0"/>"#).is_err());
    assert!(parse_rich_html(r#"<tg-map lat="0.0" lon="-181.0"/>"#).is_err());

    // Non-finite coordinates
    assert!(parse_rich_html(r#"<tg-map lat="NaN" lon="0.0"/>"#).is_err());
    assert!(parse_rich_html(r#"<tg-map lat="0.0" lon="Infinity"/>"#).is_err());

    // Zoom out of bounds [1, 20]
    assert!(parse_rich_html(r#"<tg-map lat="0.0" lon="0.0" zoom="0"/>"#).is_err());
    assert!(parse_rich_html(r#"<tg-map lat="0.0" lon="0.0" zoom="21"/>"#).is_err());
}

#[test]
fn test_parse_rich_html_rejects_collage_with_audio_or_docs() {
    let mixed =
        r#"<tg-collage><img src="tg://photo?id=p1"/><audio src="tg://audio?id=a1"/></tg-collage>"#;
    assert!(parse_rich_html(mixed).is_err());

    let mixed_doc = r#"<tg-collage><img src="tg://photo?id=p1"/><tg-document src="tg://document?id=d1"/></tg-collage>"#;
    assert!(parse_rich_html(mixed_doc).is_err());
}

#[test]
fn test_parse_rich_html_media_deduplication() {
    let html = r#"<p>Dua kali foto sama:</p><img src="tg://photo?id=pic1"/><img src="tg://photo?id=pic1"/>"#;
    let (blocks, media) = parse_rich_html(html).expect("dedup html");
    assert_eq!(blocks.len(), 3);
    assert_eq!(
        media.len(),
        1,
        "Duplicate ID must be deduplicated in media array"
    );
    assert_eq!(media[0].id, "pic1");
}

#[test]
fn test_parse_rich_html_conflicting_duplicate_id_rejected() {
    let html = r#"<img src="tg://photo?id=pic1"/><img id="pic1" src="https://example.com/different.jpg"/>"#;
    assert!(parse_rich_html(html).is_err());
}

#[test]
fn test_parse_rich_html_composite_single_unified_bubble() {
    let commentary_html = r#"<h3>Eksplorasi Gunung Rinjani</h3><p>Gunung Rinjani di Pulau Lombok adalah gunung berapi kedua tertinggi di Indonesia (3.726 mdpl) yang terkenal dengan kaldera megah dan danau kawah Segara Anak.</p><tg-collage caption="Pemandangan Kaldera & Segara Anak"><img src="tg://photo?id=pic_rinjani_1"/><img src="tg://photo?id=pic_rinjani_2"/></tg-collage><p>Berikut lokasi geografis puncak Rinjani pada peta satelit:</p><tg-map lat="-8.4113" lon="116.4573" zoom="13" title="Puncak Rinjani 3.726 mdpl"/>"#;

    let (blocks, media) = parse_rich_html(commentary_html).expect("rinjani composite rich html");

    assert_eq!(blocks.len(), 5);
    assert!(matches!(
        blocks[0],
        RichBlock::SectionHeading { level: 3, .. }
    ));
    assert!(matches!(blocks[1], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[2], RichBlock::Collage { .. }));
    assert!(matches!(blocks[3], RichBlock::Paragraph { .. }));
    assert!(matches!(blocks[4], RichBlock::Map { .. }));

    assert_eq!(media.len(), 2);
    assert_eq!(media[0].id, "pic_rinjani_1");
    assert_eq!(media[1].id, "pic_rinjani_2");
}

#[test]
fn test_resolve_media_references() {
    let html = r#"<img src="tg://photo?id=pic1" alt="Rinjani"/>"#;
    let (mut blocks, _media) = parse_rich_html(html).expect("parse rich html");

    // External resolution: media item target is remote URL
    let resolved_media = vec![InputRichMessageMedia {
        id: "pic1".to_string(),
        media: InputMedia::photo("https://example.com/resolved_rinjani.jpg", None, None),
    }];

    resolve_media_references(&mut blocks, &resolved_media);

    let Some(RichBlock::Photo { photo, .. }) = blocks.first() else {
        panic!("Expected RichBlock::Photo");
    };
    assert_eq!(photo["media"], "https://example.com/resolved_rinjani.jpg");
}

#[test]
fn test_parse_rich_html_container_degradation() {
    let collage_one = r#"<tg-collage><img src="tg://photo?id=single1"/></tg-collage>"#;
    let (blocks, _) = parse_rich_html(collage_one).expect("degrade collage 1");
    assert_eq!(blocks.len(), 1);
    assert!(
        matches!(blocks[0], RichBlock::Photo { .. }),
        "Collage with 1 item degrades to Photo"
    );

    let slideshow_one = r#"<tg-slideshow><img src="tg://photo?id=single2"/></tg-slideshow>"#;
    let (blocks, _) = parse_rich_html(slideshow_one).expect("degrade slideshow 1");
    assert_eq!(blocks.len(), 1);
    assert!(
        matches!(blocks[0], RichBlock::Photo { .. }),
        "Slideshow with 1 item degrades to Photo"
    );
}

#[test]
fn test_markdown_parser_converts_document_tag_to_rich_block() {
    let md = "Berikut laporannya:\n\n[document: test.pdf](attach://doc_0)";
    let blocks = parse_streaming_markdown_to_rich_blocks(md);

    assert_eq!(blocks.len(), 2);

    // First block is a paragraph
    if let RichBlock::Paragraph { text, .. } = &blocks[0] {
        assert_eq!(text, "Berikut laporannya:");
    } else {
        panic!("Expected Paragraph, got {:?}", blocks[0]);
    }

    // Second block is the document
    if let RichBlock::Document {
        document, caption, ..
    } = &blocks[1]
    {
        assert_eq!(document["type"], "document");
        assert_eq!(document["media"], "attach://doc_0");
        let cap = caption.as_ref().expect("caption must be present");
        assert_eq!(cap.text, serde_json::json!("test.pdf"));
    } else {
        panic!("Expected Document, got {:?}", blocks[1]);
    }
}

#[test]
fn test_markdown_parser_converts_task_list_to_checklist_rich_block() {
    let md = "- [ ] Task belum selesai\n- [x] Task sudah selesai\n- [X] Task selesai huruf besar";
    let blocks = parse_markdown_to_rich_blocks(md);
    assert_eq!(blocks.len(), 1);
    let RichBlock::List { items } = &blocks[0] else {
        panic!("Expected List block, got {:?}", blocks[0]);
    };
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].has_checkbox, Some(true));
    assert_eq!(items[0].is_checked, Some(false));
    assert_eq!(items[1].has_checkbox, Some(true));
    assert_eq!(items[1].is_checked, Some(true));
    assert_eq!(items[2].has_checkbox, Some(true));
    assert_eq!(items[2].is_checked, Some(true));
}

// ---------------------------------------------------------------------------
// Audit regressions: adversarial input must never panic, and common Markdown
// constructs must keep their meaning.
// ---------------------------------------------------------------------------

#[test]
fn html_heading_with_length_changing_lowercase_does_not_panic() {
    // `İ`.to_lowercase() is one byte longer than `İ`; the heading parser used
    // to slice the original with offsets from the lowercased copy.
    let blocks = parse_markdown_to_rich_blocks("<h1>İİİİİİ</h1>");
    let Some(RichBlock::SectionHeading { text, .. }) = blocks.first() else {
        panic!("expected heading, got {blocks:?}");
    };
    assert_eq!(text, &Value::String("İİİİİİ".to_string()));

    let multiline = parse_markdown_to_rich_blocks("<h2>\nİİİİİİİİ judul</h2>\nisi");
    assert!(multiline
        .iter()
        .any(|block| matches!(block, RichBlock::SectionHeading { .. })));
}

#[test]
fn html_attribute_lookup_with_length_changing_lowercase_does_not_panic() {
    let tag = "<a İİİİİİİİİİ href=x>";
    assert_eq!(extract_html_attribute(tag, "href"), Some("x"));
    let img = "<img alt=\"İİİİİİİİİİ\" src=\"https://example.com/a.jpg\">";
    assert_eq!(
        extract_html_attribute(img, "src"),
        Some("https://example.com/a.jpg")
    );
}

#[test]
fn italic_span_keeps_nested_bold() {
    let value = parse_inline("*a **b** c*");
    let serialized = serde_json::to_string(&value).expect("serialize inline");
    assert_eq!(
        serialized,
        r#"{"text":["a ",{"text":"b","type":"bold"}," c"],"type":"italic"}"#
    );
}

#[test]
fn backslash_escapes_render_literal_markers() {
    let value = parse_inline(r"ini \*literal\* dan harga \$5");
    assert_eq!(
        value,
        Value::String("ini *literal* dan harga $5".to_string())
    );
    // `\(` still opens inline math.
    let math = serde_json::to_string(&parse_inline(r"rumus \(x^2\)")).expect("serialize");
    assert!(math.contains("mathematical_expression"));
}

#[test]
fn escaped_entities_are_decoded_only_once_inside_nested_spans() {
    let value = parse_inline("tulis **&amp;lt;b&amp;gt;** literal");
    let serialized = serde_json::to_string(&value).expect("serialize inline");
    assert!(
        serialized.contains("&lt;b&gt;"),
        "nested span must keep the single-decoded entity: {serialized}"
    );
}

#[test]
fn deeply_layered_markup_is_bounded() {
    fn escape(s: &str, times: usize) -> String {
        let mut out = s.to_string();
        for _ in 0..times {
            out = out
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
        }
        out
    }
    let mut input = String::from("x");
    for level in (0..200).rev() {
        input = format!("{}{}{}", escape("<b>", level), input, escape("</b>", level));
    }
    let started = std::time::Instant::now();
    let _ = parse_inline(&input);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "layered input must not trigger repeated re-decoding"
    );

    let mut nested = String::new();
    let delimiters = ["**", "~~", "||", "++", "__"];
    for index in 0..10_000 {
        nested.push_str(delimiters[index % delimiters.len()]);
    }
    let _ = parse_inline(&nested);
}

#[test]
fn exponent_operator_is_not_bold() {
    let value = parse_inline("a**2 + b**2 = c**2");
    assert_eq!(value, Value::String("a**2 + b**2 = c**2".to_string()));
    let streaming = parse_streaming_markdown_to_rich_blocks("Rumus: a**2 + b**2 = c**2 dan kita");
    let serialized = serde_json::to_string(&streaming).expect("serialize blocks");
    assert!(serialized.contains("a**2 + b**2 = c**2"), "{serialized}");
    assert!(!serialized.contains("\"bold\""));
    // Regular bold still works right next to exponents.
    let mixed = serde_json::to_string(&parse_inline("**tebal** dan x**2")).expect("serialize");
    assert!(mixed.contains("\"bold\""));
    assert!(mixed.contains("x**2"));
}

fn inline_json(text: &str) -> String {
    parse_inline(text).to_string()
}

#[test]
fn highlight_superscript_and_subscript_become_rich_text_entities() {
    assert_eq!(
        parse_inline("ini ==penting== sekali"),
        serde_json::json!(["ini ", {"type": "marked", "text": "penting"}, " sekali"])
    );
    assert_eq!(
        parse_inline("<mark>stabilo</mark>"),
        serde_json::json!({"type": "marked", "text": "stabilo"})
    );
    assert_eq!(
        parse_inline("E = mc<sup>2</sup>, H<sub>2</sub>O"),
        serde_json::json!([
            "E = mc", {"type": "superscript", "text": "2"},
            ", H", {"type": "subscript", "text": "2"}, "O"
        ])
    );
    // Comparisons are not highlights, and nothing is left behind by tags
    // that never close.
    assert_eq!(inline_json("jika a == b == c"), "\"jika a == b == c\"");
    assert_eq!(inline_json("x<sup>2"), "\"x2\"");
}

#[test]
fn date_times_become_entities_only_with_a_known_moment() {
    let tagged = parse_inline(
        "Rapat <time datetime=\"2026-10-05T14:00:00+07:00\" format=\"wDT\">5 Okt 14.00 WIB</time>",
    );
    assert_eq!(
        tagged,
        serde_json::json!(["Rapat ", {
            "type": "date_time", "text": "5 Okt 14.00 WIB",
            "unix_time": 1_791_183_600_i64, "date_time_format": "wDT"
        }])
    );
    let link = parse_inline("![22:45 besok](tg://time?unix=1647531900&format=r)");
    assert_eq!(link["type"], "date_time");
    assert_eq!(link["unix_time"], 1_647_531_900_i64);
    assert_eq!(link["date_time_format"], "r");

    // Without a time zone the moment is ambiguous; an invalid format is
    // rejected by Telegram. Both keep only their text.
    assert_eq!(
        inline_json("<time datetime=\"2026-10-05 14:00\">5 Okt</time>"),
        "\"5 Okt\""
    );
    assert_eq!(
        inline_json("![besok](tg://time?unix=1647531900&format=xyz)"),
        "\"besok\""
    );
}

#[test]
fn custom_emoji_keeps_its_alternative_emoji() {
    let emoji = parse_inline("Mantap ![👍](tg://emoji?id=5368324170671202286)");
    assert_eq!(
        emoji,
        serde_json::json!(["Mantap ", {
            "type": "custom_emoji",
            "custom_emoji_id": "5368324170671202286",
            "alternative_text": "👍"
        }])
    );
    assert_eq!(
        parse_inline("<tg-emoji emoji-id=\"5368324170671202286\">🔥</tg-emoji>")["type"],
        "custom_emoji"
    );
    assert_eq!(inline_json("![👍](tg://emoji?id=bukan-angka)"), "\"👍\"");
}

#[test]
fn tags_inside_inline_code_stay_literal() {
    assert_eq!(
        parse_inline("tulis `<sup>2</sup>` atau `<b>x</b>`"),
        serde_json::json!([
            "tulis ", {"type": "code", "text": "<sup>2</sup>"},
            " atau ", {"type": "code", "text": "<b>x</b>"}
        ])
    );
}

#[test]
fn plain_channels_get_a_readable_flattening() {
    assert_eq!(
        flatten_extended_inline(
            "x<sup>2</sup> ==penting== H<sub>2</sub>O <time datetime=\"2026-10-05T14:00+07:00\">5 Okt</time> ![👍](tg://emoji?id=1) log<sub>b</sub>"
        ),
        "x² **penting** H₂O 5 Okt 👍 log_b"
    );
    assert_eq!(
        flatten_extended_inline_outside_code("`==a==` ==b=="),
        "`==a==` **b**"
    );
}

#[test]
fn full_messages_keep_extended_entities_inline() {
    let message = build_full_rich_message(
        "Rapat <time datetime=\"2026-10-05T14:00:00+07:00\">5 Okt</time> membahas x<sup>2</sup> dan <mark>anggaran</mark> ![22:45](tg://time?unix=1647531900&format=t).",
        None,
    );
    assert_eq!(message.blocks.len(), 1, "one paragraph, no media block");
    let json = serde_json::to_string(&message.blocks).expect("blocks serialize");
    assert!(json.contains(r#""type":"date_time""#), "{json}");
    assert!(json.contains(r#""unix_time":1791183600"#), "{json}");
    assert!(json.contains(r#""unix_time":1647531900"#), "{json}");
    assert!(json.contains(r#""type":"superscript""#), "{json}");
    assert!(json.contains(r#""type":"marked""#), "{json}");
    assert!(!json.contains("tg://time"), "{json}");
}
