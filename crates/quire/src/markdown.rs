use futures::{FutureExt as _, StreamExt as _};
use gpui::{App, AppContext, BorrowAppContext as _, Context, Entity, Global, WeakEntity};
use language::{Buffer, Language, LanguageMatcher, LanguageRegistry, LoadedLanguage};
use std::sync::Arc;
use theme::ActiveTheme;

struct MarkdownLanguages {
    registry: Arc<LanguageRegistry>,
    markdown: Arc<Language>,
    buffers: Vec<WeakEntity<Buffer>>,
}
impl Global for MarkdownLanguages {}

/// Languages that fenced code blocks highlight through the Markdown grammar's injections: a Zed
/// language directory in the `grammars` crate, and fence tags it accepts beyond its name and file
/// suffixes. The extra tags follow rt's highlight.js aliases.
const CODE_LANGUAGES: &[(&str, &[&str])] = &[
    ("javascript", &[]),
    ("typescript", &[]),
    ("tsx", &[]),
    ("json", &[]),
    ("jsonc", &[]),
    ("css", &[]),
    ("html", &["xml"]),
    ("yaml", &[]),
    ("python", &[]),
    ("bash", &["shell"]),
    ("rust", &[]),
    ("cpp", &[]),
    ("jsdoc", &[]),
    ("regex", &[]),
];

fn code_grammars() -> [(&'static str, tree_sitter::Language); 13] {
    [
        ("tsx", tree_sitter_typescript::LANGUAGE_TSX.into()),
        (
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        ),
        ("json", tree_sitter_json::LANGUAGE.into()),
        ("jsonc", tree_sitter_json::LANGUAGE.into()),
        ("css", tree_sitter_css::LANGUAGE.into()),
        ("html", tree_sitter_html::LANGUAGE.into()),
        ("yaml", tree_sitter_yaml::LANGUAGE.into()),
        ("python", tree_sitter_python::LANGUAGE.into()),
        ("bash", tree_sitter_bash::LANGUAGE.into()),
        ("rust", tree_sitter_rust::LANGUAGE.into()),
        ("cpp", tree_sitter_cpp::LANGUAGE.into()),
        ("jsdoc", tree_sitter_jsdoc::LANGUAGE.into()),
        ("regex", tree_sitter_regex::LANGUAGE.into()),
    ]
}

/// Registers the code languages lazily, as Zed does: a fence's language loads on the background
/// executor the first time a page uses it, and pages waiting on it parse again once it loads.
fn register_code_languages(registry: &LanguageRegistry) {
    registry.register_native_grammars(code_grammars());
    for &(name, tags) in CODE_LANGUAGES {
        let mut config = grammars::load_config(name);
        config.matcher = Arc::new(LanguageMatcher {
            path_suffixes: config
                .matcher
                .path_suffixes
                .iter()
                .cloned()
                .chain(tags.iter().map(|&tag| tag.into()))
                .collect(),
            first_line_pattern: config.matcher.first_line_pattern.clone(),
            modeline_aliases: config.matcher.modeline_aliases.clone(),
        });
        registry.register_language(
            config.name.clone(),
            config.grammar.clone(),
            config.matcher.clone(),
            config.hidden,
            None,
            Arc::new(move || {
                let config = config.clone();
                async move {
                    Ok(LoadedLanguage {
                        config,
                        queries: grammars::load_queries(name),
                        context_provider: None,
                        toolchain_provider: None,
                        manifest_name: None,
                    })
                }
                .boxed()
            }),
        );
    }
}

/// Parses again every open page that injects a language the registry has just loaded.
fn reparse_when_languages_load(registry: &LanguageRegistry, cx: &mut App) {
    let mut changes = registry.subscribe();
    cx.spawn(async move |cx| {
        while changes.next().await.is_some() {
            cx.update(|cx| {
                let buffers = cx.update_global(|languages: &mut MarkdownLanguages, _| {
                    languages
                        .buffers
                        .retain(|buffer| buffer.upgrade().is_some());
                    languages.buffers.clone()
                });
                for buffer in buffers.iter().filter_map(WeakEntity::upgrade) {
                    buffer.update(cx, |buffer, cx| {
                        if buffer.contains_unknown_injections() {
                            buffer.reparse(cx, false);
                        }
                    });
                }
            });
        }
    })
    .detach();
}

pub fn init(cx: &mut App) -> Result<(), String> {
    let registry = Arc::new(LanguageRegistry::new(cx.background_executor().clone()));
    let markdown = Arc::new(
        Language::new(grammars::load_config("markdown"), Some(tree_sitter_quire_markdown::LANGUAGE.into()))
            .with_queries(grammars::load_queries("markdown"))
            .map_err(|error| format!("Cannot load Quire's Markdown queries: {error}. Rebuild the bundled grammar resources."))?,
    );
    let inline = Arc::new(
        Language::new(grammars::load_config("markdown-inline"), Some(tree_sitter_quire_markdown::INLINE_LANGUAGE.into()))
            .with_queries(grammars::load_queries("markdown-inline"))
            .map_err(|error| format!("Cannot load Quire's inline Markdown queries: {error}. Rebuild the bundled grammar resources."))?,
    );
    registry.add(markdown.clone());
    registry.add(inline);
    register_code_languages(&registry);
    registry.set_theme(cx.theme().clone());
    let theme_registry = registry.clone();
    cx.observe_global::<theme::GlobalTheme>(move |cx| {
        theme_registry.set_theme(cx.theme().clone());
    })
    .detach();
    reparse_when_languages_load(&registry, cx);
    cx.set_global(MarkdownLanguages {
        registry,
        markdown,
        buffers: Vec::new(),
    });
    Ok(())
}

pub fn buffer(text: impl Into<String>, cx: &mut App) -> Entity<Buffer> {
    new_buffer(|cx| Buffer::local(text, cx), cx)
}

pub fn prepared_buffer(
    text: rope::Rope,
    line_ending: text::LineEnding,
    cx: &mut App,
) -> Entity<Buffer> {
    new_buffer(|cx| Buffer::local_normalized(text, line_ending, cx), cx)
}

fn new_buffer(create: impl FnOnce(&mut Context<Buffer>) -> Buffer, cx: &mut App) -> Entity<Buffer> {
    let languages = cx.global::<MarkdownLanguages>();
    let registry = languages.registry.clone();
    let markdown = languages.markdown.clone();
    let buffer = cx.new(|cx| {
        let mut buffer = create(cx);
        buffer.set_language_registry(registry);
        buffer.set_language(Some(markdown), cx);
        buffer
    });
    cx.update_global(|languages: &mut MarkdownLanguages, _| {
        languages
            .buffers
            .retain(|buffer| buffer.upgrade().is_some());
        languages.buffers.push(buffer.downgrade());
    });
    buffer
}

/// Parses `text` as Quire Markdown without creating a buffer entity. The returned future does the
/// parse and may run on the background executor.
pub fn detached_snapshot(
    text: &str,
    cx: &mut App,
) -> impl std::future::Future<Output = language::BufferSnapshot> + Send + use<> {
    let languages = cx.global::<MarkdownLanguages>();
    let (registry, markdown) = (languages.registry.clone(), languages.markdown.clone());
    Buffer::build_snapshot(text.into(), Some(markdown), Some(registry), None, cx)
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
