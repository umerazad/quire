use quire_core::daily::{
    DailyCalendar, DailyLayout, DailyParser, detect_layout, format_daily, parse_daily,
};
use serde_json::Value;

fn layout(value: &Value) -> DailyLayout {
    DailyLayout {
        folder: value["folder"].as_str().unwrap().into(),
        title: value["title"].as_str().unwrap().into(),
    }
}

#[test]
fn daily_dates_and_injected_clocks_match_isolated_node_timezones() {
    let zones: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/daily-timezone-vectors.json"
    ))
    .unwrap();
    assert_eq!(zones.len(), 8);
    for zone in zones {
        let calendar = DailyCalendar::named(zone["zone"].as_str().unwrap()).unwrap();
        for case in zone["formatted"].as_array().unwrap() {
            let layout = layout(&case["layout"]);
            let iso = case["iso"].as_str().unwrap();
            let title = case["title"].as_str().unwrap();
            assert_eq!(
                format_daily(&layout.title, iso, &calendar).unwrap(),
                title,
                "{} {case}",
                zone["zone"]
            );
            assert_eq!(
                layout.path(iso, &calendar).unwrap(),
                case["path"],
                "{} {case}",
                zone["zone"]
            );
            assert_eq!(
                parse_daily(title, &layout.title, &calendar).as_deref(),
                case["parsed"].as_str(),
                "{} {case}",
                zone["zone"]
            );
            assert_eq!(
                DailyParser::new(&layout, calendar.clone())
                    .date(title)
                    .as_deref(),
                case["date"].as_str(),
                "{} {case}",
                zone["zone"]
            );
        }
        for case in zone["parsed"].as_array().unwrap() {
            let parser = DailyParser::new(&layout(&case["layout"]), calendar.clone());
            assert_eq!(
                parser.date(case["title"].as_str().unwrap()).as_deref(),
                case["date"].as_str(),
                "{} {case}",
                zone["zone"]
            );
        }
        for case in zone["clocks"].as_array().unwrap() {
            assert_eq!(
                calendar
                    .date_at(case["milliseconds"].as_i64().unwrap())
                    .unwrap(),
                case["date"],
                "{} {case}",
                zone["zone"]
            );
        }
        let detection = &zone["detection"];
        assert_eq!(
            detect_layout(
                detection["paths"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|path| path.as_str().unwrap()),
                &calendar
            ),
            (!detection["layout"].is_null()).then(|| layout(&detection["layout"])),
            "{zone}"
        );
    }
}

#[test]
fn invalid_zones_and_out_of_range_clocks_are_actionable_errors() {
    assert!(
        DailyCalendar::named("Quire/Unknown")
            .err()
            .unwrap()
            .contains("time zone")
    );
    assert!(DailyCalendar::utc().date_at(i64::MAX).is_err());
    assert!(DailyCalendar::utc().date_at(i64::MIN).is_err());
}
