//! Formats the corpus with the Rust dprint-family crates, configured as close to
//! Prettier 3 defaults (printWidth 80, tabWidth 2, double quotes, semicolons,
//! trailing commas "all") as their documented options allow.
use std::path::Path;
use std::time::Instant;

use dprint_plugin_typescript::configuration as ts_cfg;

struct Formatters {
    ts: ts_cfg::Configuration,
    json: dprint_plugin_json::configuration::Configuration,
    md: dprint_plugin_markdown::configuration::Configuration,
    css: malva::config::FormatOptions,
    html: markup_fmt::config::FormatOptions,
    yaml: pretty_yaml::config::FormatOptions,
}

impl Formatters {
    fn new() -> Self {
        let ts = ts_cfg::ConfigurationBuilder::new()
            .line_width(80)
            .indent_width(2)
            .quote_style(ts_cfg::QuoteStyle::AlwaysDouble)
            .jsx_quote_style(ts_cfg::JsxQuoteStyle::PreferDouble)
            .semi_colons(ts_cfg::SemiColons::Always)
            .trailing_commas(ts_cfg::TrailingCommas::OnlyMultiLine)
            .arrow_function_use_parentheses(ts_cfg::UseParentheses::Force)
            .quote_props(ts_cfg::QuoteProps::AsNeeded)
            .operator_position(ts_cfg::OperatorPosition::SameLine)
            // dprint defaults that rewrite code beyond layout; Prettier never does these.
            .use_braces(ts_cfg::UseBraces::Maintain)
            .module_sort_import_declarations(ts_cfg::SortOrder::Maintain)
            .module_sort_export_declarations(ts_cfg::SortOrder::Maintain)
            .jsx_attributes_prefer_single_line(true)
            .build();
        let json = dprint_plugin_json::configuration::ConfigurationBuilder::new()
            .line_width(80)
            .indent_width(2)
            .build();
        let md = dprint_plugin_markdown::configuration::ConfigurationBuilder::new()
            .line_width(80)
            .build();
        Self {
            ts,
            json,
            md,
            css: Default::default(),
            html: Default::default(),
            yaml: Default::default(),
        }
    }

    fn script(&self, text: &str, ext: &str) -> Result<String, String> {
        let path = format!("snippet.{ext}");
        dprint_plugin_typescript::format_text(dprint_plugin_typescript::FormatTextOptions {
            path: Path::new(&path),
            extension: None,
            text: text.to_string(),
            config: &self.ts,
            external_formatter: None,
        })
        .map(|o| o.unwrap_or_else(|| text.to_string()))
        .map_err(|e| e.to_string())
    }

    fn css(&self, text: &str) -> Result<String, String> {
        malva::format_text(text, malva::Syntax::Css, &self.css).map_err(|e| format!("{e:?}"))
    }

    fn format(&self, text: &str, lang: &str) -> Result<String, String> {
        match lang {
            "js" | "javascript" | "jsx" => self.script(text, "jsx"),
            "ts" | "typescript" => self.script(text, "ts"),
            "tsx" => self.script(text, "tsx"),
            "json" => dprint_plugin_json::format_text(Path::new("snippet.json"), text, &self.json)
                .map(|o| o.unwrap_or_else(|| text.to_string()))
                .map_err(|e| e.to_string()),
            "css" => self.css(text),
            "html" | "xml" => markup_fmt::format_text(
                text,
                markup_fmt::Language::Html,
                &self.html,
                |code: &str, hints: markup_fmt::Hints| {
                    let r = match hints.ext {
                        "js" | "mjs" | "ts" | "jsx" | "tsx" => self.script(code, hints.ext),
                        "css" => self.css(code),
                        _ => Ok(code.to_string()),
                    };
                    r.map(std::borrow::Cow::Owned).map_err(anyhow::Error::msg)
                },
            )
            .map_err(|e| format!("{e:?}")),
            "yaml" | "yml" => {
                pretty_yaml::format_text(text, &self.yaml).map_err(|e| format!("{e:?}"))
            }
            "md" | "markdown" => {
                dprint_plugin_markdown::format_text(text, &self.md, |tag, code, _w| {
                    Ok(self.format(code, tag).ok())
                })
                .map(|o| o.unwrap_or_else(|| text.to_string()))
                .map_err(|e| e.to_string())
            }
            other => Err(format!("no formatter for {other}")),
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let corpus = args
        .next()
        .expect("usage: fmt_dprint <corpus_dir> <out_dir> [repeats=50]");
    let out = args.next().expect("out dir");
    let repeats: usize = args.next().map_or(50, |s| s.parse().unwrap());
    std::fs::create_dir_all(&out).unwrap();

    let t = Instant::now();
    let f = Formatters::new();
    let init_ms = t.elapsed().as_secs_f64() * 1000.0;

    let mut entries: Vec<_> = std::fs::read_dir(&corpus)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    let mut timings = Vec::new();
    for path in entries {
        let lang = path.extension().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let t = Instant::now();
        let first = f.format(&text, &lang);
        let first_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        for _ in 0..repeats {
            let _ = f.format(&text, &lang);
        }
        let warm_ms = t.elapsed().as_secs_f64() * 1000.0 / repeats as f64;
        let body = first.unwrap_or_else(|e| format!("ERROR: {}\n", e.lines().next().unwrap_or("")));
        std::fs::write(Path::new(&out).join(path.file_name().unwrap()), body).unwrap();
        timings.push(format!(
            "\"{}\":[{first_ms:.3},{warm_ms:.3}]",
            path.file_stem().unwrap().to_string_lossy()
        ));
    }
    println!(
        "{{\"tool\":\"dprint-family\",\"init_ms\":{init_ms:.2},\"timings_first_warm_ms\":{{{}}}}}",
        timings.join(",")
    );
}
