use editor::Editor;
use gpui::{App, AppContext, Entity, Focusable, KeyBinding, UpdateGlobal, Window, prelude::*};
use search::{BufferSearchBar, buffer_search::DivRegistrar};

fn init(cx: &mut App) -> Result<(), String> {
    settings::init(cx);
    let result = settings::SettingsStore::update_global(cx, |store, cx| {
        store.set_user_settings(
            r#"{"vim_mode":true,"session":{"restore_unsaved_buffers":false}}"#,
            cx,
        )
    });
    if let settings::ParseStatus::Failed { error } = result.parse_status {
        return Err(format!(
            "Cannot initialize Quire editor settings: {error}. Check the bundled defaults."
        ));
    }
    theme_settings::init(theme::LoadThemes::JustBase, cx);
    crate::markdown::init(cx)?;
    release_channel::init(semver::Version::new(0, 1, 0), cx);
    editor::init(cx);
    vim::init_with_register_retention(cx);
    cx.on_action(|_: &super::Quit, cx| cx.quit());
    cx.bind_keys([KeyBinding::new(
        if cfg!(target_os = "macos") {
            "cmd-q"
        } else {
            "ctrl-q"
        },
        super::Quit,
        None,
    )]);
    cx.bind_keys([
        KeyBinding::new("enter", editor::actions::Newline, Some("Editor")),
        KeyBinding::new("backspace", editor::actions::Backspace, Some("Editor")),
        KeyBinding::new("delete", editor::actions::Delete, Some("Editor")),
        KeyBinding::new("left", editor::actions::MoveLeft, Some("Editor")),
        KeyBinding::new("right", editor::actions::MoveRight, Some("Editor")),
        KeyBinding::new("up", zed_actions::editor::MoveUp, Some("Editor")),
        KeyBinding::new("down", zed_actions::editor::MoveDown, Some("Editor")),
        KeyBinding::new("tab", editor::actions::Tab, Some("Editor")),
        KeyBinding::new("shift-tab", editor::actions::Outdent, Some("Editor")),
    ]);
    let shortcuts = if cfg!(target_os = "macos") {
        ["cmd-z", "cmd-shift-z", "cmd-a", "cmd-c", "cmd-x", "cmd-v"]
    } else {
        [
            "ctrl-z",
            "ctrl-shift-z",
            "ctrl-a",
            "ctrl-c",
            "ctrl-x",
            "ctrl-v",
        ]
    };
    cx.bind_keys([
        KeyBinding::new(shortcuts[0], editor::actions::Undo, Some("Editor")),
        KeyBinding::new(shortcuts[1], editor::actions::Redo, Some("Editor")),
        KeyBinding::new(shortcuts[2], editor::actions::SelectAll, Some("Editor")),
        KeyBinding::new(shortcuts[3], editor::actions::Copy, Some("Editor")),
        KeyBinding::new(shortcuts[4], editor::actions::Cut, Some("Editor")),
        KeyBinding::new(shortcuts[5], editor::actions::Paste, Some("Editor")),
    ]);
    let mut keymap =
        settings::KeymapFile::parse(settings::vim_keymap().as_ref()).map_err(|error| {
            format!("Cannot read the bundled Vim bindings: {error}. Rebuild or reinstall Quire.")
        })?;
    keymap.retain_sections(|section| editor_vim_context(&section.context));
    match keymap.load_keymap(cx) {
        settings::KeymapFileLoadResult::Success { key_bindings } => cx.bind_keys(key_bindings),
        settings::KeymapFileLoadResult::SomeFailedToLoad { error_message, .. } => {
            return Err(format!(
                "Cannot load Quire Vim bindings: {error_message:?}. Rebuild or reinstall Quire."
            ));
        }
        settings::KeymapFileLoadResult::JsonParseFailure { error } => {
            return Err(format!(
                "Cannot parse Quire Vim bindings: {error}. Rebuild or reinstall Quire."
            ));
        }
    }
    crate::editor_search::bind_keys(cx);
    crate::editor_command::bind_keys(cx);
    Ok(())
}

fn editor_vim_context(context: &str) -> bool {
    context.starts_with("vim_operator == ") && !context.contains("helix")
        || [
            "VimControl && !menu",
            "vim_mode == normal || vim_mode == visual || vim_mode == operator",
            "vim_mode == normal",
            "VimControl && VimCount",
            "vim_mode == visual",
            "vim_mode == insert",
            "showing_completions",
            "(vim_mode == normal || vim_mode == helix_normal) && !menu",
            "Editor && mode == full && VimControl && vim_mode == normal && !menu && os == windows",
            "vim_mode == insert && !(showing_code_actions || showing_completions)",
            "(vim_mode == insert || vim_mode == normal) && showing_signature_help && !showing_completions",
            "vim_mode == replace",
            "vim_mode == waiting",
            "Editor && vim_mode == waiting && (vim_operator == ys || vim_operator == cs)",
            "vim_mode == operator",
            "vim_operator == a || vim_operator == i || vim_operator == cs || vim_operator == helix_next || vim_operator == helix_previous",
            "vim_mode == literal",
            "BufferSearchBar && !in_replace",
        ].contains(&context)
}

fn editor(
    buffer: Entity<language::Buffer>,
    window: &mut Window,
    cx: &mut gpui::Context<Editor>,
) -> Editor {
    Editor::for_buffer(buffer, None, window, cx)
}

struct NotebookWindow {
    source: Entity<Editor>,
    second: Entity<Editor>,
    source_search: Entity<BufferSearchBar>,
    second_search: Entity<BufferSearchBar>,
    mode: Entity<vim::ModeIndicator>,
    commands: Entity<crate::editor_command::CommandPrompt>,
}

impl NotebookWindow {
    fn open_command(
        &mut self,
        _: &crate::editor_command::OpenCommand,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let editor = if self.source.focus_handle(cx).contains_focused(window, cx) {
            &self.source
        } else if self.second.focus_handle(cx).contains_focused(window, cx) {
            &self.second
        } else {
            cx.propagate();
            return;
        };
        self.commands
            .update(cx, |prompt, cx| prompt.open(editor, window, cx));
    }
    fn search_submit(
        &mut self,
        _: &vim::SearchSubmit,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let editor = if self.source_search.focus_handle(cx).is_focused(window) {
            Some(&self.source)
        } else if self.second_search.focus_handle(cx).is_focused(window) {
            Some(&self.second)
        } else {
            None
        };
        if editor.is_none_or(|editor| !vim::submit_search(editor, window, cx)) {
            cx.propagate();
        }
    }

    #[cfg(test)]
    fn new(text: &str, window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
        let buffer = crate::markdown::buffer(text, cx);
        Self::for_buffer(buffer, None, window, cx)
    }

    fn for_buffer(
        buffer: Entity<language::Buffer>,
        excerpt: Option<Entity<multi_buffer::MultiBuffer>>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let mode = cx.new(|cx| vim::ModeIndicator::new(window, cx));
        let source = cx.new(|cx| editor(buffer.clone(), window, cx));
        let second = cx.new(|cx| match excerpt {
            Some(excerpt) => Editor::for_multibuffer(excerpt, None, window, cx),
            None => editor(buffer, window, cx),
        });
        let source_search = crate::editor_search::new(&source, window, cx);
        let second_search = crate::editor_search::new(&second, window, cx);
        let commands = cx.new(|cx| crate::editor_command::CommandPrompt::new(window, cx));
        cx.observe(&commands, |_, _, cx| cx.notify()).detach();
        cx.observe(&source_search, |_, _, cx| cx.notify()).detach();
        cx.observe(&second_search, |_, _, cx| cx.notify()).detach();
        window.focus(&source.focus_handle(cx), cx);
        Self {
            source,
            second,
            source_search,
            second_search,
            commands,
            mode,
        }
    }
}

impl Render for NotebookWindow {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let mut source =
            DivRegistrar::new(|this: &Self, _, _| Some(this.source_search.clone()), cx);
        BufferSearchBar::register(&mut source);
        let source = source
            .into_div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .when(!self.source_search.read(cx).is_dismissed(), |div| {
                div.child(self.source_search.clone())
            })
            .child(gpui::div().flex_1().min_h_0().child(self.source.clone()));
        let mut second =
            DivRegistrar::new(|this: &Self, _, _| Some(this.second_search.clone()), cx);
        BufferSearchBar::register(&mut second);
        let second = second
            .into_div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .when(!self.second_search.read(cx).is_dismissed(), |div| {
                div.child(self.second_search.clone())
            })
            .child(gpui::div().flex_1().min_h_0().child(self.second.clone()));
        gpui::div()
            .size_full()
            .on_action(cx.listener(Self::search_submit))
            .on_action(cx.listener(Self::open_command))
            .flex()
            .flex_col()
            .child(gpui::div().p_3().child("Welcome"))
            .child(
                gpui::div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(source)
                    .child(second),
            )
            .when(self.commands.read(cx).is_open(), |div| {
                div.child(self.commands.clone())
            })
            .child(gpui::div().p_2().child(self.mode.clone()))
    }
}

pub fn run() {
    let excerpt_demo = std::env::args().any(|argument| argument == "--excerpt-harness");
    gpui_platform::application().with_assets(assets::Assets).run(move |cx| {
        if let Err(error) = init(cx) {
            eprintln!("{error}");
            cx.quit();
            return;
        }
        if let Err(error) = assets::Assets.load_fonts(cx) {
            eprintln!("Cannot load Quire's bundled fonts: {error}. Rebuild the application assets.");
            cx.quit();
            return;
        }
        let prefix = "# Welcome to Quire\n\n";
        let block = "- Edit this note in either pane. ^b-welcome\n  - Both panes share the same document.";
        let text = format!("{prefix}{block}\n- [ ] Try Vim, undo, and redo.\n");
        let buffer = crate::markdown::buffer(&text, cx);
        let excerpt = if excerpt_demo {
            match crate::editor_excerpt::buffer(buffer.clone(), prefix.len()..prefix.len() + block.len(), cx) {
                Ok(excerpt) => Some(excerpt),
                Err(error) => { eprintln!("{error}"); cx.quit(); return; }
            }
        } else { None };
        let options = gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(None, gpui::size(gpui::px(1100.), gpui::px(760.)), cx))),
            ..Default::default()
        };
        match cx.open_window(options, move |window, cx| {
            window.set_window_title("Quire Native");
            cx.new(|cx| NotebookWindow::for_buffer(buffer, excerpt, window, cx))
        }) {
            Ok(_) => cx.activate(true),
            Err(error) => { eprintln!("Cannot open Quire's window: {error}. Check desktop and graphics access, then retry."); cx.quit(); }
        }
        cx.on_window_closed(|cx, _| { if cx.windows().is_empty() { cx.quit(); } }).detach();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn window_hosts_two_editors_and_a_mode_indicator(cx: &mut TestAppContext) {
        cx.update(|cx| init(cx).expect("editor initialization"));
        let window = cx.add_window(|window, cx| NotebookWindow::new("hello", window, cx));
        cx.simulate_keystrokes(window.into(), "i x escape");
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.source.read(cx).text(cx), "xhello");
                assert_eq!(view.second.read(cx).text(cx), "xhello");
            })
            .unwrap();
    }

    #[gpui::test]
    fn notebook_window_parses_markdown_and_inline_syntax_in_its_shared_buffer(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| init(cx).expect("editor initialization"));
        let text = "# Quire\n\n- **مرحبا** [link](https://example.com) ^b-1\n";
        let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
        cx.executor().run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let source = view
                    .source
                    .read(cx)
                    .buffer()
                    .read(cx)
                    .as_singleton()
                    .unwrap();
                let second = view
                    .second
                    .read(cx)
                    .buffer()
                    .read(cx)
                    .as_singleton()
                    .unwrap();
                assert_eq!(source.entity_id(), second.entity_id());
                let snapshot = source.read(cx).snapshot();
                assert_eq!(snapshot.language().unwrap().name().as_ref(), "Markdown");
                let layers: Vec<_> = snapshot.syntax_layers().collect();
                assert!(
                    layers
                        .iter()
                        .any(|layer| layer.node().to_sexp().contains("atx_heading"))
                );
                assert!(
                    layers
                        .iter()
                        .any(|layer| layer.node().to_sexp().contains("list_item"))
                );
                assert!(
                    layers
                        .iter()
                        .any(|layer| layer.language.name().as_ref() == "Markdown-Inline"
                            && layer.node().to_sexp().contains("strong_emphasis"))
                );
                assert_eq!(view.source.read(cx).text(cx), text);
                assert_eq!(view.second.read(cx).text(cx), text);
            })
            .unwrap();
    }

    #[gpui::test]
    fn markdown_reparses_shared_vim_edits_and_undo_from_the_other_pane(cx: &mut TestAppContext) {
        cx.update(|cx| init(cx).expect("editor initialization"));
        let original = "# Quire\n\n**مرحبا**\n";
        let window = cx.add_window(|window, cx| NotebookWindow::new(original, window, cx));
        cx.simulate_keystrokes(window.into(), "d d");
        cx.executor().run_until_parked();
        window
            .update(cx, |view, window, cx| {
                assert_eq!(view.source.read(cx).text(cx), "\n**مرحبا**\n");
                assert_eq!(view.second.read(cx).text(cx), "\n**مرحبا**\n");
                let buffer = view
                    .source
                    .read(cx)
                    .buffer()
                    .read(cx)
                    .as_singleton()
                    .unwrap();
                assert!(
                    !buffer
                        .read(cx)
                        .snapshot()
                        .syntax_layers()
                        .any(|layer| layer.node().to_sexp().contains("atx_heading"))
                );
                window.focus(&view.second.focus_handle(cx), cx);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "u");
        cx.executor().run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.source.read(cx).text(cx), original);
                assert_eq!(view.second.read(cx).text(cx), original);
                let buffer = view
                    .source
                    .read(cx)
                    .buffer()
                    .read(cx)
                    .as_singleton()
                    .unwrap();
                assert!(
                    buffer
                        .read(cx)
                        .snapshot()
                        .syntax_layers()
                        .any(|layer| layer.node().to_sexp().contains("atx_heading"))
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn hosts_an_editor_without_a_project(cx: &mut TestAppContext) {
        cx.update(|cx| init(cx).expect("editor initialization"));
        let buffer = cx.new(|cx| language::Buffer::local("# Quire\n\n- A thought ^b-1\n", cx));
        let window = cx.add_window(|window, cx| editor(buffer.clone(), window, cx));
        window
            .update(cx, |editor, _, cx| {
                assert_eq!(editor.text(cx), "# Quire\n\n- A thought ^b-1\n");
            })
            .unwrap();
    }

    #[gpui::test]
    fn vim_keyboard_editing_shares_the_buffer(cx: &mut TestAppContext) {
        cx.update(|cx| init(cx).expect("editor initialization"));
        let buffer = cx.new(|cx| language::Buffer::local("hello", cx));
        let source = cx.add_window(|window, cx| {
            let editor = editor(buffer.clone(), window, cx);
            window.focus(&editor.focus_handle(cx), cx);
            editor
        });
        let other = cx.add_window(|window, cx| editor(buffer, window, cx));
        cx.simulate_keystrokes(source.into(), "i x escape");
        other
            .update(cx, |editor, _, cx| assert_eq!(editor.text(cx), "xhello"))
            .unwrap();
        other
            .update(cx, |editor, window, cx| {
                window.focus(&editor.focus_handle(cx), cx)
            })
            .unwrap();
        cx.simulate_keystrokes(other.into(), "u");
        other
            .update(cx, |editor, _, cx| assert_eq!(editor.text(cx), "hello"))
            .unwrap();
        cx.simulate_keystrokes(source.into(), "ctrl-r");
        other
            .update(cx, |editor, _, cx| assert_eq!(editor.text(cx), "xhello"))
            .unwrap();
    }

    #[gpui::test]
    fn vim_operators_and_multiline_insert_work(cx: &mut TestAppContext) {
        cx.update(|cx| init(cx).expect("editor initialization"));
        let buffer = cx.new(|cx| language::Buffer::local("one\ntwo\n", cx));
        let source = cx.add_window(|window, cx| {
            let editor = editor(buffer, window, cx);
            window.focus(&editor.focus_handle(cx), cx);
            editor
        });
        cx.simulate_keystrokes(source.into(), "d d");
        source
            .update(cx, |editor, _, cx| assert_eq!(editor.text(cx), "two\n"))
            .unwrap();
        cx.simulate_keystrokes(source.into(), "i x enter y escape");
        source
            .update(cx, |editor, _, cx| assert_eq!(editor.text(cx), "x\nytwo\n"))
            .unwrap();
    }

    #[gpui::test]
    fn normal_editing_and_clipboard_work_when_vim_is_disabled(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init(cx).expect("editor initialization");
            let result = settings::SettingsStore::update_global(cx, |store, cx| {
                store.set_user_settings(
                    r#"{"vim_mode":false,"session":{"restore_unsaved_buffers":false}}"#,
                    cx,
                )
            });
            assert!(!matches!(
                result.parse_status,
                settings::ParseStatus::Failed { .. }
            ));
        });
        let buffer = cx.new(|cx| language::Buffer::local("", cx));
        let source = cx.add_window(|window, cx| {
            let editor = editor(buffer, window, cx);
            window.focus(&editor.focus_handle(cx), cx);
            editor
        });
        cx.simulate_keystrokes(source.into(), "a enter b");
        source
            .update(cx, |editor, _, cx| assert_eq!(editor.text(cx), "a\nb"))
            .unwrap();
        let shortcuts = if cfg!(target_os = "macos") {
            "cmd-a cmd-c right cmd-v"
        } else {
            "ctrl-a ctrl-c right ctrl-v"
        };
        cx.simulate_keystrokes(source.into(), shortcuts);
        source
            .update(cx, |editor, _, cx| assert_eq!(editor.text(cx), "a\nba\nb"))
            .unwrap();
    }
}

#[cfg(test)]
#[path = "editor_search_tests.rs"]
mod search_tests;

#[cfg(test)]
#[path = "editor_command_tests.rs"]
mod command_tests;

#[cfg(test)]
#[path = "editor_input_tests.rs"]
mod input_tests;

#[cfg(test)]
#[path = "editor_excerpt_tests.rs"]
mod excerpt_tests;
