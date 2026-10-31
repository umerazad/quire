//! Runs the unmodified Prettier 3.9.10 standalone bundles inside QuickJS (rquickjs),
//! with the options rt 0.9.57 uses, and records init and per-call latency.
use rquickjs::{Context, Function, Object, Promise, Runtime};
use std::path::Path;
use std::time::Instant;

macro_rules! bundle {
    ($p:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../node_modules/prettier/",
            $p
        ))
    };
}

const BUNDLES: [&str; 8] = [
    bundle!("standalone.js"),
    bundle!("plugins/babel.js"),
    bundle!("plugins/estree.js"),
    bundle!("plugins/typescript.js"),
    bundle!("plugins/postcss.js"),
    bundle!("plugins/html.js"),
    bundle!("plugins/markdown.js"),
    bundle!("plugins/yaml.js"),
];

const GLUE: &str = r#"
globalThis.__format = (text, parser) =>
  prettier.format(text, { parser, plugins: Object.values(prettierPlugins), tabWidth: 2 });
"#;

fn parser(lang: &str) -> Option<&'static str> {
    Some(match lang {
        "js" | "javascript" | "jsx" => "babel",
        "ts" | "typescript" | "tsx" => "typescript",
        "json" => "json",
        "css" => "css",
        "html" | "xml" => "html",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        _ => return None,
    })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let corpus = args
        .next()
        .expect("usage: fmt_quickjs <corpus_dir> <out_dir> [repeats=10]");
    let out = args.next().expect("out dir");
    let repeats: usize = args.next().map_or(10, |s| s.parse().unwrap());
    std::fs::create_dir_all(&out).unwrap();

    let t = Instant::now();
    let rt = Runtime::new().unwrap();
    rt.set_max_stack_size(8 << 20);
    let ctx = Context::full(&rt).unwrap();
    ctx.with(|ctx| {
        for src in BUNDLES.iter().chain([GLUE].iter()) {
            ctx.eval::<(), _>(*src).unwrap();
        }
    });
    let init_ms = t.elapsed().as_secs_f64() * 1000.0;

    let format = |text: &str, parser: &str| -> Result<String, String> {
        ctx.with(|ctx| {
            let f: Function = ctx.globals().get("__format").map_err(|e| e.to_string())?;
            let p: Promise = f.call((text, parser)).map_err(|e| e.to_string())?;
            p.finish::<String>().map_err(|e| {
                let exc = ctx.catch();
                exc.as_object()
                    .and_then(|o: &Object| o.get::<_, String>("message").ok())
                    .unwrap_or_else(|| e.to_string())
            })
        })
    };

    let mut entries: Vec<_> = std::fs::read_dir(&corpus)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    let mut timings = Vec::new();
    for path in entries {
        let lang = path.extension().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let Some(parser) = parser(&lang) else {
            continue;
        };
        let t = Instant::now();
        let first = format(&text, parser);
        let first_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        for _ in 0..repeats {
            let _ = format(&text, parser);
        }
        let warm_ms = t.elapsed().as_secs_f64() * 1000.0 / repeats as f64;
        let body = first.unwrap_or_else(|e| format!("ERROR: {}\n", e.lines().next().unwrap_or("")));
        std::fs::write(Path::new(&out).join(path.file_name().unwrap()), body).unwrap();
        timings.push(format!(
            "\"{}\":[{first_ms:.2},{warm_ms:.2}]",
            path.file_name().unwrap().to_string_lossy()
        ));
    }
    let mem = rt.memory_usage();
    println!(
        "{{\"tool\":\"prettier-3.9.10-in-quickjs\",\"init_ms\":{init_ms:.1},\"js_heap_mb\":{:.1},\"timings_first_warm_ms\":{{{}}}}}",
        mem.memory_used_size as f64 / 1048576.0,
        timings.join(",")
    );
}
