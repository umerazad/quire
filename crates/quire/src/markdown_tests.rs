use gpui::{Entity, TestAppContext};
use language::{Buffer, LanguageAwareStyling};
use theme::ActiveTheme as _;

/// One fence per tag rt highlights, the token checked inside it, the language Zed resolves the
/// tag to, and the theme highlight the token receives.
const FENCES: &[(&str, &str, &str, &str, &str)] = &[
    ("js", "const x = 1", "const", "JavaScript", "keyword"),
    ("javascript", "let y = 'a'", "'a'", "JavaScript", "string"),
    ("jsx", "const el = <div />", "div", "JavaScript", "tag"),
    ("ts", "type T = string", "type", "TypeScript", "keyword"),
    (
        "typescript",
        "let n: number = 2",
        "number",
        "TypeScript",
        "type",
    ),
    ("tsx", "const t = <span>x</span>", "span", "TSX", "tag"),
    ("json", "{\"key\": true}", "true", "JSON", "boolean"),
    ("css", ".a { color: red; }", "color", "CSS", "property"),
    (
        "html",
        "<p class=\"x\">hi</p>",
        "class",
        "HTML",
        "attribute",
    ),
    ("xml", "<note><to>Tove</to></note>", "note", "HTML", "tag"),
    ("yaml", "name: quire", "name", "YAML", "property"),
    ("yml", "flag: true", "true", "YAML", "boolean"),
    ("py", "def run(): pass", "def", "Python", "keyword"),
    ("sh", "echo \"hi\"", "\"hi\"", "Shell Script", "string"),
    (
        "shell",
        "if true; then echo; fi",
        "if",
        "Shell Script",
        "keyword",
    ),
    ("rust", "fn main() {}", "fn", "Rust", "keyword"),
    (
        "c",
        "int main(void) { return 0; }",
        "return",
        "C++",
        "keyword",
    ),
    ("cpp", "class A {};", "class", "C++", "keyword"),
    ("md", "# Title", "Title", "Markdown", "title"),
];

fn page() -> String {
    FENCES
        .iter()
        .map(|(tag, code, ..)| format!("```{tag}\n{code}\n```\n\n"))
        .collect()
}

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).expect("editor initialization"));
}

/// Waits until every fence's language has loaded and the page has parsed again with it.
fn wait_for_injections(buffer: &Entity<Buffer>, cx: &mut TestAppContext) {
    for _ in 0..50 {
        cx.executor().run_until_parked();
        let ready = buffer.read_with(cx, |buffer, _| !buffer.contains_unknown_injections());
        if ready {
            return;
        }
    }
    panic!("the fenced code languages did not load; check the code language registrations");
}

fn highlight_at(
    buffer: &Entity<Buffer>,
    offset: usize,
    len: usize,
    cx: &TestAppContext,
) -> (String, String) {
    cx.read(|cx| {
        let snapshot = buffer.read(cx).snapshot();
        let language = snapshot
            .language_at(offset)
            .map(|language| language.name().0.to_string())
            .unwrap_or_default();
        let id = snapshot
            .chunks(
                offset..offset + len,
                LanguageAwareStyling {
                    tree_sitter: true,
                    diagnostics: false,
                },
            )
            .find_map(|chunk| chunk.syntax_highlight_id);
        let name = id
            .and_then(|id| cx.theme().syntax().get_capture_name(id))
            .unwrap_or("none")
            .to_string();
        (language, name)
    })
}

#[gpui::test]
fn fenced_code_highlights_through_markdown_injections(cx: &mut TestAppContext) {
    init(cx);
    let text = page();
    let buffer = cx.update(|cx| crate::markdown::buffer(text.clone(), cx));
    wait_for_injections(&buffer, cx);
    let mut fence_start = 0;
    let actual: Vec<_> = FENCES
        .iter()
        .map(|(tag, code, token, ..)| {
            let fence = text[fence_start..].find(&format!("```{tag}\n")).unwrap() + fence_start;
            let code_start = fence + tag.len() + 4;
            fence_start = code_start + code.len();
            let offset = code_start + code.find(token).unwrap();
            let (language, name) = highlight_at(&buffer, offset, token.len(), cx);
            (*tag, language, name)
        })
        .collect();
    let expected: Vec<_> = FENCES
        .iter()
        .map(|(tag, _, _, language, name)| (*tag, language.to_string(), name.to_string()))
        .collect();
    assert_eq!(actual, expected);
}

#[gpui::test]
fn unknown_fence_tags_stay_plain_and_editable(cx: &mut TestAppContext) {
    init(cx);
    let text = "```clojure\n(defn f [] 1)\n```\n";
    let buffer = cx.update(|cx| crate::markdown::buffer(text, cx));
    cx.executor().run_until_parked();
    let offset = text.find("defn").unwrap();
    let (language, _) = highlight_at(&buffer, offset, 4, cx);
    assert_eq!(language, "Markdown");
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(offset..offset, "x")], None, cx)
    });
    cx.executor().run_until_parked();
    assert_eq!(
        buffer.read_with(cx, |buffer, _| buffer.text()),
        "```clojure\n(xdefn f [] 1)\n```\n"
    );
}

#[gpui::test]
fn a_fence_typed_after_opening_highlights_once_its_language_loads(cx: &mut TestAppContext) {
    init(cx);
    let buffer = cx.update(|cx| crate::markdown::buffer("Notes\n", cx));
    cx.executor().run_until_parked();
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(6..6, "```python\nimport os\n```\n")], None, cx)
    });
    wait_for_injections(&buffer, cx);
    let offset = "Notes\n```python\n".len();
    assert_eq!(
        highlight_at(&buffer, offset, "import".len(), cx),
        ("Python".to_string(), "keyword".to_string())
    );
}

/// The color a view paints on `token`, from its display chunks: an editor highlight wins over the
/// syntax highlight, as when the editor paints them.
fn view_color(
    window: gpui::WindowHandle<editor::Editor>,
    token: &str,
    cx: &mut TestAppContext,
) -> Option<gpui::Hsla> {
    window
        .update(cx, |editor, _, cx| {
            let snapshot = editor.display_snapshot(cx);
            let rows = editor::display_map::DisplayRow(0)
                ..editor::display_map::DisplayRow(snapshot.max_point().row().0 + 1);
            let syntax = cx.theme().syntax().clone();
            let mut text = String::new();
            let mut colors = Vec::new();
            for chunk in snapshot.chunks(
                rows,
                LanguageAwareStyling {
                    tree_sitter: true,
                    diagnostics: false,
                },
                editor::display_map::HighlightStyles::default(),
            ) {
                let color = chunk
                    .highlight_style
                    .and_then(|style| style.color)
                    .or_else(|| {
                        chunk
                            .syntax_highlight_id
                            .and_then(|id| syntax.get(id)?.color)
                    });
                colors.push((text.len()..text.len() + chunk.text.len(), color));
                text.push_str(chunk.text);
            }
            let at = text.find(token)?;
            colors
                .into_iter()
                .find(|(range, _)| range.contains(&at))
                .and_then(|(_, color)| color)
        })
        .unwrap()
}

#[gpui::test]
fn write_and_source_paint_fenced_code_with_the_theme_syntax_colors(cx: &mut TestAppContext) {
    use gpui::{AppContext as _, Focusable as _};
    init(cx);
    let text = "- Item\n  ```js\n  const x = 1\n  ```\n\n```css\n.a { color: red; }\n```\n";
    let buffer = cx.update(|cx| crate::markdown::buffer(text, cx));
    wait_for_injections(&buffer, cx);
    let open = |write: bool, cx: &mut TestAppContext| {
        let buffer = buffer.clone();
        let window = cx.add_window(|window, cx| {
            let editor = editor::Editor::for_buffer(buffer, None, window, cx);
            window.focus(&editor.focus_handle(cx), cx);
            editor
        });
        if write {
            let editor = window.root(cx).unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                crate::editor_projection::bind(&editor, window, cx).expect("Write projection")
            })
            .unwrap();
        }
        cx.executor().run_until_parked();
        window
    };
    let source = open(false, cx);
    let write = open(true, cx);
    for (token, name) in [("const", "keyword"), ("color", "property")] {
        let color = cx
            .read(|cx| cx.theme().syntax().style_for_name(name))
            .and_then(|style| style.color)
            .unwrap_or_else(|| panic!("the theme has no `{name}` color"));
        assert_eq!(view_color(source, token, cx), Some(color), "Source {token}");
        assert_eq!(view_color(write, token, cx), Some(color), "Write {token}");
    }
}
