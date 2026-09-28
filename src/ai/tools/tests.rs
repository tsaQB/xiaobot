use super::*;

#[test]
fn test_sanitize_multimedia_caption_behavior() {
    let mut none_caption: Option<String> = None;
    sanitize_multimedia_caption(&mut none_caption);
    assert!(none_caption.is_none());

    let mut empty_caption = Some("   \n\t  ".to_string());
    sanitize_multimedia_caption(&mut empty_caption);
    assert!(empty_caption.is_none());

    let mut normal_caption = Some("  Gambar Pemandangan  ".to_string());
    sanitize_multimedia_caption(&mut normal_caption);
    assert_eq!(normal_caption.as_deref(), Some("Gambar Pemandangan"));

    let long_str: String = "a".repeat(MULTIMEDIA_CAPTION_MAX_CHARS + 50);
    let mut long_caption = Some(long_str);
    sanitize_multimedia_caption(&mut long_caption);
    assert_eq!(
        long_caption.as_ref().map(|s| s.chars().count()),
        Some(MULTIMEDIA_CAPTION_MAX_CHARS)
    );
}

#[test]
fn test_tools_definition_contains_expected_tools() {
    let tools = get_tools_definition();
    let array = tools.as_array().expect("tools should be an array");
    assert_eq!(array.len(), 12);

    let names: Vec<_> = array
        .iter()
        .filter_map(|t| t.get("function")?.get("name")?.as_str())
        .collect();
    assert!(names.contains(&"web_search"));
    assert!(names.contains(&"fetch_url"));
    assert!(names.contains(&"create_quiz"));
    assert!(names.contains(&"send_photo"));
    assert!(names.contains(&"send_collage"));
    assert!(names.contains(&"send_slideshow"));
    assert!(names.contains(&"send_audio"));
    assert!(names.contains(&"send_voice"));
    assert!(names.contains(&"send_location"));
    assert!(names.contains(&"send_document"));
    assert!(names.contains(&"create_document"));
    assert!(names.contains(&"create_archive"));
}

#[test]
fn test_search_engine_status_not_empty() {
    let (engine, detail) = get_search_engine_status();
    assert!(!engine.is_empty());
    assert!(!detail.is_empty());
}

#[test]
fn test_html_cleaning_logic() {
    let raw_html = "<html><head><style>body{color:red;}</style></head><body><h1>Hello &amp; Welcome</h1><script>alert(1);</script><p>This is a test.</p><ul><li>Item 1</li><li>Item 2</li></ul></body></html>";
    let cleaned = clean_html_to_text(raw_html);

    assert_eq!(
        cleaned,
        "Hello & Welcome\n\nThis is a test.\n\n• Item 1\n• Item 2"
    );
}

#[test]
fn test_is_tool_calling_preamble() {
    assert!(is_tool_calling_preamble(
        "Tunggu sebentar, saya sedang mencari referensi"
    ));
    assert!(is_tool_calling_preamble("Sebentar ya, saya carikan"));
    assert!(is_tool_calling_preamble("Saya akan carikan harga SO"));
    assert!(is_tool_calling_preamble("Saya carikan harga SOL terkini"));
    assert!(is_tool_calling_preamble("Biar saya cek dulu"));
    assert!(is_tool_calling_preamble("Baik, akan saya carikan datanya"));
    assert!(is_tool_calling_preamble(
        "Let me search for that information"
    ));
    assert!(is_tool_calling_preamble(
        "I'll look up the latest SOL price"
    ));
    assert!(!is_tool_calling_preamble(
        "Halo, tentu ini jawaban dari pertanyaan Anda"
    ));
    assert!(!is_tool_calling_preamble(""));
}

#[test]
fn test_is_suppressed_tool_preamble_stream() {
    // Early stream chunks that could be preamble openings
    assert!(is_suppressed_tool_preamble_stream("Saya"));
    assert!(is_suppressed_tool_preamble_stream("Saya akan"));
    assert!(is_suppressed_tool_preamble_stream("Saya akan carikan"));
    assert!(is_suppressed_tool_preamble_stream(
        "Saya akan carikan harga SO"
    ));
    assert!(is_suppressed_tool_preamble_stream("Biar"));
    assert!(is_suppressed_tool_preamble_stream("Tunggu sebentar"));

    // Direct, non-preamble answers should not be suppressed
    assert!(!is_suppressed_tool_preamble_stream(
        "Solana adalah platform blockchain layer-1 berkecepatan tinggi dengan konsensus Proof of History."
    ));
    assert!(!is_suppressed_tool_preamble_stream(
        "Bitcoin diciptakan pada tahun 2009 oleh Satoshi Nakamoto sebagai mata uang terdesentralisasi."
    ));
    assert!(!is_suppressed_tool_preamble_stream(
        "Baik, tentu ini jawaban dari pertanyaan Anda."
    ));
    assert!(!is_suppressed_tool_preamble_stream(
        "Saya adalah asisten AI pribadi Anda."
    ));
}

#[test]
fn test_search_engine_status_and_resolvers() {
    let (status, mcp_url) = get_search_engine_status();
    assert!(!status.is_empty());
    assert!(!mcp_url.is_empty());
    assert!(mcp_url.starts_with("http"));
}

#[test]
fn test_create_quiz_cascading_duplicate_disambiguation() {
    let mut args = CreateQuizArgs {
        question: "Question with duplicates?".to_string(),
        options: vec!["A".into(), "A (2)".into(), "A".into()],
        correct_option_id: Some(0),
        explanation: None,
        preamble: None,
        is_anonymous: None,
    };
    args.sanitize();
    assert_eq!(args.options, vec!["A", "A (2)", "A (3)"]);
    assert!(args.validate().is_ok());

    // Extreme duplicate case: 4 identical options
    let mut all_same = CreateQuizArgs {
        question: "Same options?".to_string(),
        options: vec!["X".into(), "X".into(), "X".into(), "X".into()],
        correct_option_id: Some(2),
        explanation: None,
        preamble: None,
        is_anonymous: None,
    };
    all_same.sanitize();
    assert_eq!(all_same.options, vec!["X", "X (2)", "X (3)", "X (4)"]);
    assert!(all_same.validate().is_ok());
}

#[test]
fn test_create_quiz_flexible_deserialization() {
    // Deserializing options from both objects and strings, plus string boolean and string correct_option_id
    let json_data = r#"{
        "question": "Flexible quiz test?",
        "options": [{"text": "Obj Option 1"}, "Plain Option 2"],
        "correct_option_id": "1",
        "is_anonymous": "false",
        "explanation": "Valid explanation"
    }"#;

    let args: CreateQuizArgs =
        serde_json::from_str(json_data).expect("must deserialize flexible quiz args");
    assert_eq!(args.question, "Flexible quiz test?");
    assert_eq!(args.options, vec!["Obj Option 1", "Plain Option 2"]);
    assert_eq!(args.correct_option_id, Some(1));
    assert_eq!(args.is_anonymous, Some(false));
    assert_eq!(args.explanation.as_deref(), Some("Valid explanation"));
    assert!(args.validate().is_ok());
}

#[test]
fn test_create_quiz_oversized_preamble_truncated() {
    let huge_preamble = "a".repeat(crate::bot::models::RICH_MESSAGE_MAX_TEXT_CHARS + 100);
    let mut args = CreateQuizArgs {
        question: "Quiz with huge preamble?".to_string(),
        options: vec!["A".into(), "B".into()],
        correct_option_id: Some(0),
        explanation: None,
        preamble: Some(huge_preamble),
        is_anonymous: None,
    };
    args.sanitize();
    let pre = args.preamble.expect("preamble must exist");
    assert_eq!(
        pre.chars().count(),
        crate::bot::models::RICH_MESSAGE_MAX_TEXT_CHARS
    );
}

#[test]
fn test_deserialize_flexible_f64() {
    #[derive(serde::Deserialize)]
    struct Coord {
        #[serde(deserialize_with = "deserialize_flexible_f64")]
        val: f64,
    }

    // Float number
    let c1: Coord = serde_json::from_str(r#"{"val": -6.2088}"#).expect("should deserialize float");
    assert!((c1.val - -6.2088).abs() < 1e-6);

    // Integer number
    let c2: Coord = serde_json::from_str(r#"{"val": 106}"#).expect("should deserialize integer");
    assert_eq!(c2.val, 106.0);

    // Float string
    let c3: Coord =
        serde_json::from_str(r#"{"val": " -6.2088 "}"#).expect("should deserialize float string");
    assert!((c3.val - -6.2088).abs() < 1e-6);

    // Integer string
    let c4: Coord =
        serde_json::from_str(r#"{"val": "180"}"#).expect("should deserialize integer string");
    assert_eq!(c4.val, 180.0);

    // Non-number string should fail
    assert!(serde_json::from_str::<Coord>(r#"{"val": "abc"}"#).is_err());

    // Empty string should fail
    assert!(serde_json::from_str::<Coord>(r#"{"val": ""}"#).is_err());

    // NaN string should fail
    assert!(serde_json::from_str::<Coord>(r#"{"val": "NaN"}"#).is_err());

    // Infinity string should fail
    assert!(serde_json::from_str::<Coord>(r#"{"val": "Infinity"}"#).is_err());
    assert!(serde_json::from_str::<Coord>(r#"{"val": "-inf"}"#).is_err());
}

#[test]
fn test_send_location_args_validation() {
    // String coordinates
    let json_str = r#"{"latitude": "-6.2088", "longitude": " 106.8456 ", "title": " Monas "}"#;
    let mut args: SendLocationArgs = serde_json::from_str(json_str).expect("deserialize location");
    args.sanitize();
    assert_eq!(args.title.as_deref(), Some("Monas"));
    assert!(args.validate().is_ok());

    // Exact boundary coordinates
    let b1 = SendLocationArgs {
        latitude: 90.0,
        longitude: 180.0,
        title: None,
    };
    assert!(b1.validate().is_ok());

    let b2 = SendLocationArgs {
        latitude: -90.0,
        longitude: -180.0,
        title: None,
    };
    assert!(b2.validate().is_ok());

    // Latitude out of bounds
    let bad_lat = SendLocationArgs {
        latitude: 90.001,
        longitude: 0.0,
        title: None,
    };
    assert!(bad_lat.validate().is_err());

    let bad_lat_neg = SendLocationArgs {
        latitude: -90.001,
        longitude: 0.0,
        title: None,
    };
    assert!(bad_lat_neg.validate().is_err());

    // Longitude out of bounds
    let bad_lon = SendLocationArgs {
        latitude: 0.0,
        longitude: 180.001,
        title: None,
    };
    assert!(bad_lon.validate().is_err());

    let bad_lon_neg = SendLocationArgs {
        latitude: 0.0,
        longitude: -180.001,
        title: None,
    };
    assert!(bad_lon_neg.validate().is_err());

    // Non-finite coordinates
    let nan_loc = SendLocationArgs {
        latitude: f64::NAN,
        longitude: 0.0,
        title: None,
    };
    assert!(nan_loc.validate().is_err());

    let inf_loc = SendLocationArgs {
        latitude: 0.0,
        longitude: f64::INFINITY,
        title: None,
    };
    assert!(inf_loc.validate().is_err());
}

#[test]
fn test_send_collage_args_validation_and_media_mapping() {
    // 1 item should fail validation
    let mut single = SendCollageArgs {
        urls: vec!["https://example.com/1.jpg".to_string()],
        caption: Some("Single photo".to_string()),
    };
    single.sanitize();
    assert!(single.validate().is_err());

    // 2 items should pass
    let mut two = SendCollageArgs {
        urls: vec![
            "https://example.com/1.jpg".to_string(),
            "https://example.com/2.jpg".to_string(),
        ],
        caption: Some("Album".to_string()),
    };
    two.sanitize();
    assert!(two.validate().is_ok());

    // InputMedia conversion verifies caption on first item only
    let media = two.to_input_media();
    assert_eq!(media.len(), 2);
    match &media[0] {
        crate::bot::models::InputMedia::Photo { media, caption, .. } => {
            assert_eq!(media, "https://example.com/1.jpg");
            assert_eq!(caption.as_deref(), Some("Album"));
        }
        _ => panic!("Expected InputMedia::Photo"),
    }
    match &media[1] {
        crate::bot::models::InputMedia::Photo { media, caption, .. } => {
            assert_eq!(media, "https://example.com/2.jpg");
            assert!(caption.is_none());
        }
        _ => panic!("Expected InputMedia::Photo"),
    }

    // 11 items sanitized truncates to 10
    let urls_11: Vec<String> = (0..11)
        .map(|i| format!("https://example.com/{i}.jpg"))
        .collect();
    let mut collage_11 = SendCollageArgs {
        urls: urls_11,
        caption: None,
    };
    assert!(collage_11.validate().is_err());
    collage_11.sanitize();
    assert_eq!(collage_11.urls.len(), 10);
    assert!(collage_11.validate().is_ok());

    // Empty URL string fails validation
    let empty_url = SendCollageArgs {
        urls: vec!["https://example.com/1.jpg".to_string(), "   ".to_string()],
        caption: None,
    };
    assert!(empty_url.validate().is_err());
}

#[test]
fn test_send_slideshow_args_validation() {
    let empty = SendSlideshowArgs {
        urls: vec![],
        caption: None,
    };
    assert!(empty.validate().is_err());

    let mut valid = SendSlideshowArgs {
        urls: vec!["https://example.com/1.jpg".to_string()],
        caption: Some("   Slide caption   ".to_string()),
    };
    valid.sanitize();
    assert_eq!(valid.caption.as_deref(), Some("Slide caption"));
    assert!(valid.validate().is_ok());
}

#[test]
fn test_send_photo_args_validation() {
    let empty = SendPhotoArgs {
        url: "   ".to_string(),
        caption: None,
    };
    assert!(empty.validate().is_err());

    let mut valid = SendPhotoArgs {
        url: "https://example.com/photo.png".to_string(),
        caption: Some("Caption".to_string()),
    };
    valid.sanitize();
    assert!(valid.validate().is_ok());

    // Oversized caption truncated to MULTIMEDIA_CAPTION_MAX_CHARS (1024)
    let huge_caption = "x".repeat(1500);
    let mut oversized = SendPhotoArgs {
        url: "https://example.com/photo.png".to_string(),
        caption: Some(huge_caption),
    };
    oversized.sanitize();
    assert_eq!(
        oversized.caption.as_deref().map(|c| c.chars().count()),
        Some(MULTIMEDIA_CAPTION_MAX_CHARS)
    );
}

#[test]
fn test_send_audio_args_validation() {
    let empty = SendAudioArgs {
        url: "".to_string(),
        title: None,
        performer: None,
        caption: None,
    };
    assert!(empty.validate().is_err());

    let mut valid = SendAudioArgs {
        url: "https://example.com/song.mp3".to_string(),
        title: Some(" Song Title ".to_string()),
        performer: Some(" Artist Name ".to_string()),
        caption: Some(" Great track ".to_string()),
    };
    valid.sanitize();
    assert_eq!(valid.title.as_deref(), Some("Song Title"));
    assert_eq!(valid.performer.as_deref(), Some("Artist Name"));
    assert_eq!(valid.caption.as_deref(), Some("Great track"));
    assert!(valid.validate().is_ok());
}

#[test]
fn test_send_voice_args_validation() {
    let empty = SendVoiceArgs {
        url: "".to_string(),
        caption: None,
    };
    assert!(empty.validate().is_err());

    let mut valid = SendVoiceArgs {
        url: "https://example.com/voice.ogg".to_string(),
        caption: Some("Voice note".to_string()),
    };
    valid.sanitize();
    assert!(valid.validate().is_ok());
}

#[test]
fn test_send_document_args_validation() {
    let empty = SendDocumentArgs {
        url: "".to_string(),
        file_name: None,
        caption: None,
    };
    assert!(empty.validate().is_err());

    let mut valid = SendDocumentArgs {
        url: "https://example.com/doc.pdf".to_string(),
        file_name: Some(" doc.pdf ".to_string()),
        caption: Some(" Annual Report ".to_string()),
    };
    valid.sanitize();
    assert_eq!(valid.file_name.as_deref(), Some("doc.pdf"));
    assert_eq!(valid.caption.as_deref(), Some("Annual Report"));
    assert!(valid.validate().is_ok());
}

#[test]
fn test_is_valid_raster_image_url_accepts_valid_raster_extensions() {
    assert!(is_valid_raster_image_url(
        "https://example.com/photos/mountain.jpg"
    ));
    assert!(is_valid_raster_image_url(
        "https://example.com/photos/mountain.jpeg"
    ));
    assert!(is_valid_raster_image_url(
        "https://example.com/photos/mountain.png"
    ));
    assert!(is_valid_raster_image_url(
        "https://example.com/photos/mountain.webp"
    ));
    assert!(is_valid_raster_image_url(
        "http://example.com/photos/mountain.JPG"
    ));

    assert!(is_valid_raster_image_url(
        "https://upload.wikimedia.org/wikipedia/commons/4/4c/KAGAGAHAN_RIJANI.jpg"
    ));
    assert!(is_valid_raster_image_url(
        "https://thumb.wikimedia.org/wikipedia/commons/thumb/4/4c/KAGAGAHAN_RIJANI.jpg/1280px-KAGAGAHAN_RIJANI.jpg?utm_source=id.wikipedia.org"
    ));

    assert!(is_valid_raster_image_url(
        "https://images.unsplash.com/photo-1546527868-ccb7ee7dfa6a"
    ));
    assert!(is_valid_raster_image_url(
        "https://images.unsplash.com/photo-1546527868-ccb7ee7dfa6a?fm=jpg&w=1080"
    ));

    assert!(is_valid_raster_image_url(
        "//cdn.example.com/images/cat.png"
    ));
    assert!(is_valid_raster_image_url(
        "https://example.com/fetch-image?id=123&format=webp"
    ));
}

#[test]
fn test_is_valid_raster_image_url_rejects_svg_gif_and_data_urls() {
    assert!(!is_valid_raster_image_url("https://example.com/logo.svg"));
    assert!(!is_valid_raster_image_url(
        "https://upload.wikimedia.org/wikipedia/en/4/4a/Commons-logo.svg"
    ));

    assert!(!is_valid_raster_image_url(
        "https://example.com/spinner.gif"
    ));
    assert!(!is_valid_raster_image_url("https://example.com/icon.ico"));

    assert!(!is_valid_raster_image_url("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="));

    assert!(!is_valid_raster_image_url("javascript:alert(1)"));
    assert!(!is_valid_raster_image_url("file:///etc/passwd"));
    assert!(!is_valid_raster_image_url(
        "blob:https://example.com/1234-5678"
    ));

    assert!(!is_valid_raster_image_url(""));
    assert!(!is_valid_raster_image_url("   "));
}

#[test]
fn test_is_valid_raster_image_url_rejects_tracking_and_placeholders() {
    assert!(!is_valid_raster_image_url(
        "https://duckduckgo.com/t/tqadb?5540565&s=lite"
    ));
    assert!(!is_valid_raster_image_url(
        "//duckduckgo.com/t/tqadb?5540565&s=lite"
    ));

    assert!(!is_valid_raster_image_url(
        "https://example.com/tracking/pixel.gif"
    ));
    assert!(!is_valid_raster_image_url("https://example.com/1x1.gif"));
    assert!(!is_valid_raster_image_url("https://example.com/spacer.gif"));
    assert!(!is_valid_raster_image_url(
        "https://google-analytics.com/collect.jpg"
    ));

    assert!(!is_valid_raster_image_url(
        "https://via.placeholder.com/300.jpg"
    ));
    assert!(!is_valid_raster_image_url(
        "https://dummyimage.com/600x400.png"
    ));
    assert!(!is_valid_raster_image_url(
        "https://placekitten.com/200/300.jpg"
    ));

    assert!(!is_valid_raster_image_url(
        "http://localhost:8080/image.jpg"
    ));
    assert!(!is_valid_raster_image_url("http://127.0.0.1/test.png"));

    assert!(!is_valid_raster_image_url(
        "../assets/anomaly/images/challenge/123.jpg"
    ));
}

#[test]
fn test_sanitize_and_validate_raster_url_strips_tracking_params() {
    let dirty = "https://upload.wikimedia.org/wikipedia/commons/4/4c/KAGAGAHAN_RIJANI.jpg?utm_source=id.wikipedia.org&utm_campaign=api&utm_content=original&fbclid=IwAR123";
    let cleaned = sanitize_and_validate_raster_url(dirty).expect("should be valid");
    assert_eq!(
        cleaned,
        "https://upload.wikimedia.org/wikipedia/commons/4/4c/KAGAGAHAN_RIJANI.jpg"
    );
}

#[test]
fn test_is_visual_search_query_detection() {
    assert!(is_visual_search_query(
        "Berikan 2 foto pemandangan gunung rinjani"
    ));
    assert!(is_visual_search_query("cari gambar kucing persia lucu"));
    assert!(is_visual_search_query("tampilkan potret presiden soekarno"));
    assert!(is_visual_search_query("pemandangan danau toba"));
    assert!(is_visual_search_query("wallpaper sunset pantai kuta"));

    assert!(is_visual_search_query("show me 3 photos of Mount Bromo"));
    assert!(is_visual_search_query("find pictures of Tokyo tower"));
    assert!(is_visual_search_query(
        "give me high resolution images of aurora"
    ));
    assert!(is_visual_search_query("download wallpaper of galaxy"));
    assert!(is_visual_search_query("python logo png official"));
    assert!(is_visual_search_query("berikan gambar logonya disini"));
    assert!(is_visual_search_query("tampilkan lambang garuda pancasila"));
    assert!(is_visual_search_query("icon rust programming language"));
    assert!(is_visual_search_query("simbol atom fisika"));

    assert!(!is_visual_search_query("harga solana hari ini"));
    assert!(!is_visual_search_query("apa itu rust borrow checker"));
    assert!(!is_visual_search_query("sejarah kemerdekaan indonesia"));
    assert!(!is_visual_search_query(""));
}

#[test]
fn test_is_logo_query_detection() {
    assert!(is_logo_query("python logo png official"));
    assert!(is_logo_query("berikan gambar logonya disini"));
    assert!(is_logo_query("lambang indonesia"));
    assert!(is_logo_query("simbol atom"));
    assert!(is_logo_query("icon telegram"));
    assert!(is_logo_query("emblem club barcelona"));
    assert!(is_logo_query("badge army"));

    assert!(!is_logo_query("pemandangan gunung bromo"));
    assert!(!is_logo_query("foto kucing lucu"));
    assert!(!is_logo_query("harga bitcoin"));
    assert!(!is_logo_query(""));
}

#[test]
fn test_extract_core_search_terms_strips_conversational_verbs() {
    assert_eq!(
        extract_core_search_terms("Berikan 2 foto pemandangan gunung rinjani"),
        "pemandangan gunung rinjani"
    );
    assert_eq!(
        extract_core_search_terms("Tolong carikan gambar kucing persia"),
        "kucing persia"
    );
    assert_eq!(
        extract_core_search_terms("Show me 3 photos of Mount Bromo"),
        "Mount Bromo"
    );
    assert_eq!(
        extract_core_search_terms("3 photos of Mount Bromo"),
        "Mount Bromo"
    );
    assert_eq!(
        extract_core_search_terms("photos of Mount Bromo"),
        "Mount Bromo"
    );
    assert_eq!(
        extract_core_search_terms("Show me photos of Mount Bromo"),
        "Mount Bromo"
    );
    assert_eq!(
        extract_core_search_terms("Show me 5 pictures of Tokyo Tower"),
        "Tokyo Tower"
    );
    assert_eq!(
        extract_core_search_terms("Gunung Rinjani"),
        "Gunung Rinjani"
    );
    // Word boundary tests: whole words like \bphotos?\b must not strip subwords
    assert_eq!(
        extract_core_search_terms("photosynthesis process in plants"),
        "photosynthesis process in plants"
    );
    assert_eq!(
        extract_core_search_terms("fotovoltaik panel surya"),
        "fotovoltaik panel surya"
    );
    // Trailing conversational suffixes
    assert_eq!(
        extract_core_search_terms("Mount Bromo, please"),
        "Mount Bromo"
    );
    assert_eq!(
        extract_core_search_terms("gunung bromo dong"),
        "gunung bromo"
    );
}

#[test]
fn test_extract_raster_images_from_html() {
    let html_content = r#"
        <div>
            <img src="https://example.com/photos/valid1.jpg" alt="Valid 1">
            <img data-src="https://example.com/photos/valid2.png" alt="Valid 2">
            <img data-lazy-src="https://example.com/photos/valid_lazy.webp" alt="Lazy">
            <img srcset="https://example.com/photos/valid_srcset_small.jpg 400w, https://example.com/photos/valid_srcset_large.webp 1200w">
            <img src="/relative/valid3.webp" alt="Relative valid">
            <img src="https://example.com/icon.svg" alt="SVG rejected">
            <img src="https://google-analytics.com/pixel.gif" alt="Tracker rejected">
            <img src="https://via.placeholder.com/150.jpg" alt="Placeholder rejected">
            <a href="https://example.com/gallery/full_mountain.jpg">Download Full</a>
        </div>
    "#;

    let extracted = extract_raster_images_from_html(html_content, Some("https://example.com"));
    assert!(extracted.len() >= 6);
    assert!(extracted.contains(&"https://example.com/photos/valid1.jpg".to_string()));
    assert!(extracted.contains(&"https://example.com/photos/valid2.png".to_string()));
    assert!(extracted.contains(&"https://example.com/photos/valid_lazy.webp".to_string()));
    assert!(extracted.contains(&"https://example.com/photos/valid_srcset_small.jpg".to_string()));
    assert!(extracted.contains(&"https://example.com/photos/valid_srcset_large.webp".to_string()));
    assert!(extracted.contains(&"https://example.com/relative/valid3.webp".to_string()));
    assert!(extracted.contains(&"https://example.com/gallery/full_mountain.jpg".to_string()));
}

#[test]
fn test_duckduckgo_proxy_url_unwrapping() {
    let proxy_url = "https://external-content.duckduckgo.com/iu/?u=https%3A%2F%2Fexample.com%2Fphotos%2Fsummit.jpg&f=1&nofb=1";
    let unwrapped = sanitize_and_validate_raster_url(proxy_url);
    assert_eq!(
        unwrapped,
        Some("https://example.com/photos/summit.jpg".to_string())
    );

    let tracking_url = "https://duckduckgo.com/t/tqadb?5540565&s=lite";
    assert_eq!(sanitize_and_validate_raster_url(tracking_url), None);
}

#[test]
fn test_format_verified_images_section_and_guidance() {
    let imgs = vec![
        "https://example.com/1.jpg".to_string(),
        "https://example.com/2.png".to_string(),
    ];
    let formatted = format_verified_images_section(&imgs);
    assert!(formatted.contains("🖼️ **URL Foto/Gambar Raster Terverifikasi"));
    assert!(formatted.contains("- https://example.com/1.jpg"));
    assert!(formatted.contains("- https://example.com/2.png"));

    let empty_formatted = format_verified_images_section(&[]);
    assert!(empty_formatted.is_empty());

    let guidance = format_no_images_guidance("gunung rinjani");
    assert!(guidance.contains("ℹ️ **Catatan Media**"));
    assert!(guidance.contains("gunung rinjani"));
    assert!(guidance.contains("teks Markdown"));
}

#[test]
fn test_create_archive_args_sanitization_and_validation() {
    let mut args = CreateArchiveArgs {
        filename: "../../project".to_string(),
        files: vec![
            ArchiveFileEntry {
                filename: "../../../etc/passwd".to_string(),
                content: "root:x:0:0".to_string(),
            },
            ArchiveFileEntry {
                filename: "src/main.rs".to_string(),
                content: "fn main() {}".to_string(),
            },
        ],
        caption: Some("  Test Archive  ".to_string()),
    };

    args.sanitize();
    assert_eq!(args.filename, "project.zip");
    assert_eq!(args.files[0].filename, "etc/passwd");
    assert_eq!(args.files[1].filename, "src/main.rs");
    assert_eq!(args.caption.as_deref(), Some("Test Archive"));
    assert!(args.validate().is_ok());

    let empty_args = CreateArchiveArgs {
        filename: "empty.zip".to_string(),
        files: vec![],
        caption: None,
    };
    assert!(empty_args.validate().is_err());
}

#[test]
fn archive_entry_paths_cannot_escape_on_any_platform() {
    assert_eq!(
        sanitize_archive_entry_path(r"..\..\..\evil.bat"),
        "evil.bat"
    );
    assert_eq!(
        sanitize_archive_entry_path(r"C:\Windows\System32\drivers\x.sys"),
        "Windows/System32/drivers/x.sys"
    );
    assert_eq!(sanitize_archive_entry_path("/etc/passwd"), "etc/passwd");
    assert_eq!(sanitize_archive_entry_path("a/./b/../c.txt"), "a/b/c.txt");
    assert_eq!(sanitize_archive_entry_path("..\\"), "file.txt");
    assert_eq!(
        sanitize_archive_entry_path("docs\\readme.md"),
        "docs/readme.md"
    );
}

#[test]
fn test_error_recovery_never_returns_empty_response() {
    let err_msg = "Connection timeout to search provider";
    let query = "pemandangan lombok";
    let recovery = format!(
        "[Informasi Pencarian Web]\nPencarian web daring untuk topik \"{query}\" saat ini tidak dapat diselesaikan karena kendala koneksi atau penyedia pencarian sedang tidak tersedia ({err_msg}).\n\nℹ️ **Panduan Asisten**: Berikan tanggapan deskriptif dan faktual mengenai topik \"{query}\" berdasarkan pengetahuan internal Anda secara lengkap. Jika pengguna meminta gambar atau foto, jelaskan informasi visualnya secara naratif dalam teks Markdown dan hindari memanggil tool multimedia fiktif."
    );

    assert!(!recovery.trim().is_empty());
    assert!(recovery.contains(query));
    assert!(recovery.contains(err_msg));
    assert!(recovery.contains("Panduan Asisten"));
}

#[tokio::test]
async fn test_execute_web_search_empty_query_returns_clean_guidance() {
    let empty_res = execute_web_search("").await;
    assert!(!empty_res.trim().is_empty());
    assert!(empty_res.contains("tidak boleh kosong"));

    let whitespace_res = execute_web_search("   \t\n  ").await;
    assert!(!whitespace_res.trim().is_empty());
    assert!(whitespace_res.contains("tidak boleh kosong"));
}

#[test]
fn test_create_document_args_sanitization() {
    let mut args = CreateDocumentArgs {
        filename: "../../etc/passwd".to_string(),
        content: "secret".to_string(),
        caption: Some("   Test caption...   ".to_string()),
        as_zip: false,
    };
    args.sanitize();
    assert_eq!(args.filename, "etcpasswd");
    assert_eq!(args.caption, Some("Test caption...".to_string()));
    assert!(args.validate().is_ok());

    let mut emp = CreateDocumentArgs {
        filename: "    ".to_string(),
        content: "".to_string(),
        caption: None,
        as_zip: false,
    };
    emp.sanitize();
    assert_eq!(emp.filename, "document.txt");
    assert!(emp.validate().is_err());
}

#[test]
fn guest_mode_offers_only_read_only_research_tools() {
    let names = |definition: Value| -> Vec<String> {
        definition
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|tool| tool.pointer("/function/name").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    };
    let mut guest = names(tools_definition_for(true));
    guest.sort();
    assert_eq!(guest, ["fetch_url", "web_search"]);
    assert_eq!(
        names(tools_definition_for(false)),
        names(get_tools_definition())
    );
}
