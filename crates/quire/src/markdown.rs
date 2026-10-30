use gpui::{App, AppContext, Entity, Global};
use language::{Buffer, Language, LanguageRegistry};
use std::sync::Arc;
use theme::ActiveTheme;

struct MarkdownLanguages {
    registry: Arc<LanguageRegistry>,
    markdown: Arc<Language>,
}
impl Global for MarkdownLanguages {}

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
    registry.set_theme(cx.theme().clone());
    let theme_registry = registry.clone();
    cx.observe_global::<theme::GlobalTheme>(move |cx| {
        theme_registry.set_theme(cx.theme().clone());
    })
    .detach();
    cx.set_global(MarkdownLanguages { registry, markdown });
    Ok(())
}

pub fn buffer(text: impl Into<String>, cx: &mut App) -> Entity<Buffer> {
    let languages = cx.global::<MarkdownLanguages>();
    let registry = languages.registry.clone();
    let markdown = languages.markdown.clone();
    cx.new(|cx| {
        let mut buffer = Buffer::local(text, cx);
        buffer.set_language_registry(registry);
        buffer.set_language(Some(markdown), cx);
        buffer
    })
}
