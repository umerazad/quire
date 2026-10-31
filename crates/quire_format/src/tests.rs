use super::*;
use sha2::{Digest as _, Sha256};
use std::time::Instant;

fn block_on<F: Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

#[test]
fn fence_tags_choose_rts_parsers() {
    let cases = [
        ("js", Some(Parser::Babel)),
        ("JSX", Some(Parser::Babel)),
        ("javascript", Some(Parser::Babel)),
        ("ts", Some(Parser::TypeScript)),
        ("tsx", Some(Parser::TypeScript)),
        ("typescript", Some(Parser::TypeScript)),
        ("json", Some(Parser::Json)),
        ("css", Some(Parser::Css)),
        ("html", Some(Parser::Html)),
        ("xml", Some(Parser::Html)),
        ("yaml", Some(Parser::Yaml)),
        ("yml", Some(Parser::Yaml)),
        ("md", Some(Parser::Markdown)),
        ("markdown", Some(Parser::Markdown)),
        ("py", None),
        ("", None),
    ];
    for (tag, parser) in cases {
        assert_eq!(Parser::for_tag(tag), parser, "{tag}");
    }
}

#[test]
fn embedded_bundles_match_the_pinned_hashes() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../vendor/prettier/UPSTREAM.json")).unwrap();
    assert_eq!(manifest["version"], "3.9.10");
    for (path, source) in engine::BUNDLES {
        let expected = manifest["files"][path].as_str().unwrap_or_else(|| {
            panic!("vendor/prettier/UPSTREAM.json has no hash for {path}; record it")
        });
        assert_eq!(
            hex::encode(Sha256::digest(source.as_bytes())),
            expected,
            "{path} differs from the pinned Prettier 3.9.10 file"
        );
    }
}

#[test]
fn formats_rts_own_case_and_reports_syntax_errors() {
    let formatter = Formatter::default();
    assert_eq!(
        block_on(formatter.format("const x={a:1}", Parser::Babel)),
        Ok("const x = { a: 1 };\n".to_string())
    );
    let error = block_on(formatter.format("const = 1", Parser::Babel)).unwrap_err();
    assert_eq!(
        error,
        FormatError::Invalid("SyntaxError: Unexpected token (1:7)".into())
    );
    assert_eq!(
        error.to_string(),
        "Code formatting failed: SyntaxError: Unexpected token (1:7). Correct its syntax, then run Format code again."
    );
    assert_eq!(
        block_on(formatter.format("a:  1", Parser::Yaml)),
        Ok("a: 1\n".to_string()),
        "a rejected input leaves the engine usable"
    );
}

#[test]
fn refuses_oversized_input_without_starting_the_engine() {
    let formatter = Formatter::new(Limits {
        max_bytes: 4,
        ..Limits::default()
    });
    let error = block_on(formatter.format("const x = 1", Parser::Babel)).unwrap_err();
    assert_eq!(
        error,
        FormatError::TooLarge {
            bytes: 11,
            limit: 4
        }
    );
    assert!(formatter.jobs.lock().unwrap().is_none());
}

/// About 6,000 lines of JavaScript, which QuickJS takes well over a second to format.
fn large_source() -> String {
    (0..2000)
        .map(|i| format!("function f{i}(a,b){{return a+b*{i}}}\nconst v{i}={{a:[1,2,3],b:'x'}}\nf{i}(v{i}.a,2)\n"))
        .collect()
}

#[test]
fn cancelling_stops_a_running_request_and_the_next_one_succeeds() {
    let formatter = Formatter::default();
    let warm = block_on(formatter.format("x", Parser::Babel));
    assert_eq!(warm, Ok("x;\n".to_string()));
    let running = formatter.format(large_source(), Parser::Babel);
    let started = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    running.cancel();
    assert_eq!(block_on(running), Err(FormatError::Cancelled));
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "cancellation took {:?}",
        started.elapsed()
    );
    assert_eq!(
        block_on(formatter.format("let  y", Parser::Babel)),
        Ok("let y;\n".to_string())
    );
}

#[test]
fn dropping_a_queued_request_cancels_it() {
    let formatter = Formatter::default();
    let running = formatter.format(large_source(), Parser::Babel);
    drop(formatter.format("let  z", Parser::Babel));
    running.cancel();
    assert_eq!(block_on(running), Err(FormatError::Cancelled));
    assert_eq!(
        block_on(formatter.format("let  w", Parser::Babel)),
        Ok("let w;\n".to_string())
    );
}

#[test]
fn a_request_past_its_deadline_times_out() {
    let formatter = Formatter::new(Limits {
        timeout: Duration::from_millis(200),
        ..Limits::default()
    });
    let error = block_on(formatter.format(large_source(), Parser::Babel)).unwrap_err();
    assert_eq!(error, FormatError::TimedOut(Duration::from_millis(200)));
    assert_eq!(
        block_on(formatter.format("a{color:red}", Parser::Css)),
        Ok("a {\n  color: red;\n}\n".to_string())
    );
}

#[test]
fn error_messages_say_what_to_do_next() {
    assert_eq!(
        FormatError::Unsupported("py".into()).to_string(),
        "Quire has no bundled formatter for `py` code. Format it by hand; Format code supports JavaScript, TypeScript, JSON, CSS, HTML, XML, YAML, and Markdown."
    );
    assert_eq!(
        FormatError::Unsupported(String::new()).to_string(),
        "This code block has no language tag, so Quire cannot format it. Add a tag such as `js` after the opening fence, then run Format code again."
    );
    assert_eq!(
        FormatError::TooLarge {
            bytes: 600 * 1024,
            limit: 512 * 1024
        }
        .to_string(),
        "Code formatting stopped: the block is 600 KB and the formatter accepts at most 512 KB. Split the block, then run Format code again."
    );
    assert_eq!(
        FormatError::TimedOut(Duration::from_secs(20)).to_string(),
        "Code formatting stopped after 20 seconds. Split the block into smaller ones, then run Format code again."
    );
}
