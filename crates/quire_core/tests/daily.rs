use quire_core::daily::{
    DailyLayout, daily_date, detect_layout, format_daily, parse_daily, read_settings,
    write_settings,
};
use serde_json::Value;

#[test]
fn unicode_page_names_do_not_panic_during_layout_detection() {
    assert_eq!(detect_layout(["🙂", "اردو", "ی", "Folder/میٹنگ.md"]), None);
}

#[test]
fn notebook_parsers_are_isolated_and_invalid_dates_cannot_become_paths() {
    use quire_core::daily::DailyParser;
    let first = DailyLayout {
        folder: "Daily/{YYYY}".into(),
        title: "Day {YYYY}.{MM}.{DD}".into(),
    };
    let second = DailyLayout {
        folder: "Elsewhere".into(),
        title: "Date {DD} {MMMM} {YYYY}".into(),
    };
    let a = DailyParser::new(&first);
    let b = DailyParser::new(&second);
    assert_eq!(a.date("Day 2026.10.29").as_deref(), Some("2026-10-29"));
    assert_eq!(b.date("Day 2026.10.29"), None);
    assert_eq!(
        b.date("Date 29 October 2026").as_deref(),
        Some("2026-10-29")
    );
    assert_eq!(a.date("Date 29 October 2026"), None);
    for date in [
        "",
        "2026-02-30",
        "2026-13-01",
        "2026-01-00",
        "2026-1-1",
        "26-01-01",
        "2026-10-29\n",
        "٢٠٢٦-١٠-٢٩",
    ] {
        assert!(first.path(date).is_err(), "{date}");
    }
}

#[test]
fn unrepresentable_layout_settings_remain_read_only_without_losing_original_data() {
    let text =
        r#"{"version":1,"daily":{"folder":"\ud800","title":"{YYYY}-{MM}-{DD}"},"future":true}"#;
    let original = quire_core::json::Json::parse(text.as_bytes()).unwrap();
    let settings = read_settings(Some(text));
    assert!(!settings.configured);
    assert!(
        settings
            .read_only
            .unwrap()
            .contains("Preserve .quire/notebook.json")
    );
    assert_eq!(
        original
            .root()
            .field("daily")
            .field("folder")
            .as_js_string()
            .unwrap()
            .utf16_units()
            .collect::<Vec<_>>(),
        [0xd800]
    );
    assert!(original.root().field("future").as_bool().unwrap());
}

fn layout(value: &Value) -> DailyLayout {
    DailyLayout {
        folder: value["folder"].as_str().unwrap().into(),
        title: value["title"].as_str().unwrap().into(),
    }
}

#[test]
fn daily_rules_match_the_frozen_oracle() {
    let vectors: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/daily-vectors.json")).unwrap();
    assert!(vectors["formatted"].as_array().unwrap().len() >= 2000);
    for case in vectors["formatted"].as_array().unwrap() {
        let layout = layout(&case["layout"]);
        let iso = case["iso"].as_str().unwrap();
        assert_eq!(
            format_daily(&layout.title, iso).unwrap(),
            case["title"],
            "{case}"
        );
        assert_eq!(layout.folder(iso).unwrap(), case["folder"], "{case}");
        assert_eq!(layout.path(iso).unwrap(), case["path"], "{case}");
        assert_eq!(
            parse_daily(case["title"].as_str().unwrap(), &layout.title).as_deref(),
            case["parsed"].as_str(),
            "{case}"
        );
    }
    for case in vectors["parsed"].as_array().unwrap() {
        let layout = layout(&case["layout"]);
        let title = case["title"].as_str().unwrap();
        assert_eq!(
            parse_daily(title, &layout.title).as_deref(),
            case["parsed"].as_str(),
            "{case}"
        );
        assert_eq!(
            daily_date(title, &layout).as_deref(),
            case["date"].as_str(),
            "{case}"
        );
    }
    for case in vectors["problems"].as_array().unwrap() {
        let layout = layout(&case["layout"]);
        assert_eq!(layout.problem(), case["problem"].as_str(), "{case}");
        assert_eq!(write_settings(&layout), case["written"], "{case}");
    }
    for case in vectors["settings"].as_array().unwrap() {
        let settings = read_settings(case["text"].as_str());
        let expected = &case["result"];
        assert_eq!(settings.layout, layout(&expected["layout"]), "{case}");
        assert_eq!(
            settings.configured,
            expected["configured"].as_bool().unwrap(),
            "{case}"
        );
        assert_eq!(
            settings.read_only.as_deref(),
            expected["readOnly"].as_str(),
            "{case}"
        );
    }
    for case in vectors["detections"].as_array().unwrap() {
        let paths: Vec<_> = case["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        assert_eq!(
            detect_layout(paths.iter().copied()),
            (!case["layout"].is_null()).then(|| layout(&case["layout"])),
            "{case}"
        );
    }
}
