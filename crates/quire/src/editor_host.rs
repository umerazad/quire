use editor::Editor;
#[cfg(test)]
use gpui::UpdateGlobal;
use gpui::{App, AppContext, Entity, Focusable, KeyBinding, Window, prelude::*};
use search::{BufferSearchBar, buffer_search::DivRegistrar};
use ui::{Button, ButtonCommon, Clickable, Label, LabelCommon, Tooltip};

mod stored;

pub(crate) use stored::{dispatch_to_notebook, notebook_windows, open_in_profile};

gpui::actions!(quire, [TogglePresentation]);

pub(super) const EDITOR_CONTEXT: &str = "Editor && !QuireControl";

struct Initialized;

impl gpui::Global for Initialized {}

/// Initializes the editor host once per app. A second call is ignored: Vim and the editor
/// register global keystroke and entity observers, and registering them again would record
/// every Vim action twice, so `.` would replay `o` twice.
pub(super) fn init(cx: &mut App) -> Result<(), String> {
    if cx.has_global::<Initialized>() {
        return Ok(());
    }
    cx.set_global(Initialized);
    settings::init(cx);
    crate::preferences::init(cx);
    crate::ai::settings::init(cx);
    #[cfg(unix)]
    crate::notebook_tools::init(crate::notebook_tools::directory(), cx);
    crate::help::init(cx);
    crate::rt_migration::view::init(cx);
    crate::showcase::init(cx);
    theme_settings::init(theme::LoadThemes::All(Box::new(assets::Assets)), cx);
    crate::markdown::init(cx)?;
    crate::media::init(cx);
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
        KeyBinding::new("enter", editor::actions::Newline, Some(EDITOR_CONTEXT)),
        KeyBinding::new(
            "backspace",
            editor::actions::Backspace,
            Some(EDITOR_CONTEXT),
        ),
        KeyBinding::new("delete", editor::actions::Delete, Some(EDITOR_CONTEXT)),
        KeyBinding::new("left", editor::actions::MoveLeft, Some(EDITOR_CONTEXT)),
        KeyBinding::new("right", editor::actions::MoveRight, Some(EDITOR_CONTEXT)),
        KeyBinding::new("up", zed_actions::editor::MoveUp, Some(EDITOR_CONTEXT)),
        KeyBinding::new("down", zed_actions::editor::MoveDown, Some(EDITOR_CONTEXT)),
        KeyBinding::new(
            "home",
            editor::actions::MoveToBeginningOfLine {
                stop_at_soft_wraps: true,
                stop_at_indent: true,
            },
            Some(EDITOR_CONTEXT),
        ),
        KeyBinding::new(
            "end",
            editor::actions::MoveToEndOfLine {
                stop_at_soft_wraps: true,
            },
            Some(EDITOR_CONTEXT),
        ),
        KeyBinding::new("tab", editor::actions::Tab, Some(EDITOR_CONTEXT)),
        KeyBinding::new("shift-tab", editor::actions::Outdent, Some(EDITOR_CONTEXT)),
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
        KeyBinding::new(shortcuts[0], editor::actions::Undo, Some(EDITOR_CONTEXT)),
        KeyBinding::new(shortcuts[1], editor::actions::Redo, Some(EDITOR_CONTEXT)),
        KeyBinding::new(
            shortcuts[2],
            editor::actions::SelectAll,
            Some(EDITOR_CONTEXT),
        ),
        KeyBinding::new(shortcuts[3], editor::actions::Copy, Some(EDITOR_CONTEXT)),
        KeyBinding::new(shortcuts[4], editor::actions::Cut, Some(EDITOR_CONTEXT)),
        KeyBinding::new(shortcuts[5], editor::actions::Paste, Some(EDITOR_CONTEXT)),
    ]);
    let mut keymap =
        settings::KeymapFile::parse(settings::vim_keymap().as_ref()).map_err(|error| {
            format!("Cannot read the bundled Vim bindings: {error}. Rebuild or reinstall Quire.")
        })?;
    keymap.retain_sections(|section| editor_vim_context(&section.context));
    keymap.constrain_contexts("!QuireControl");
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
    crate::outline::init(cx);
    crate::block_move::init(cx);
    crate::code_format::init(cx);
    crate::task_lists::init(cx);
    crate::tasks::init(cx);
    crate::references::init(cx);
    crate::embeds::init(cx);
    crate::page_menu::init(cx);
    crate::ai::init(cx);
    crate::profiles::init(cx);
    crate::app_menus::init(cx);
    crate::app_menus::bind_keys(cx);
    stored::bind_keys(cx);
    crate::navigation::bind_keys(cx);
    crate::preferences::bind_keys(cx);
    crate::help::bind_keys(cx);
    crate::media::bind_keys(cx);
    crate::ai::bind_keys(cx);
    if !cx.has_global::<db::AppDatabase>() {
        let name = format!("quire-interface-{}", uuid::Uuid::new_v4());
        let connection = futures::executor::block_on(
            db::sqlez::thread_safe_connection::ThreadSafeConnection::builder::<db::AppMigrator>(
                &name, false,
            )
            .build(),
        )
        .map_err(|error| {
            format!("Cannot prepare Quire's interface state: {error:#}. Restart Quire; if this repeats, reinstall it.")
        })?;
        cx.set_global(db::AppDatabase(connection));
    }
    cx.bind_keys(write_and_source_keys());
    Ok(())
}

/// The reference Mod-E switch between Write and Source, Cmd/Ctrl-Shift-M as its alias, and Mod-Y
/// for redo. Off macOS Ctrl-E and Ctrl-Y scroll in Vim's Normal mode and insert the character
/// below or above in Insert mode, so there they apply only with Vim off.
fn write_and_source_keys() -> [KeyBinding; 3] {
    if cfg!(target_os = "macos") {
        [
            KeyBinding::new("cmd-e", TogglePresentation, Some(EDITOR_CONTEXT)),
            KeyBinding::new("cmd-shift-m", TogglePresentation, Some(EDITOR_CONTEXT)),
            KeyBinding::new("cmd-y", editor::actions::Redo, Some(EDITOR_CONTEXT)),
        ]
    } else {
        let without_vim = "Editor && !QuireControl && !vim_mode";
        [
            KeyBinding::new("ctrl-e", TogglePresentation, Some(without_vim)),
            KeyBinding::new("ctrl-shift-m", TogglePresentation, Some(EDITOR_CONTEXT)),
            KeyBinding::new("ctrl-y", editor::actions::Redo, Some(without_vim)),
        ]
    }
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
    let mut editor = Editor::for_buffer(buffer, None, window, cx);
    crate::page_menu::install(&mut editor);
    editor
}

pub(crate) struct NotebookWindow {
    analysis: Entity<crate::page_analysis::PageAnalysis>,
    source: Entity<Editor>,
    second: Entity<Editor>,
    source_search: Entity<BufferSearchBar>,
    second_search: Entity<BufferSearchBar>,
    mode: Entity<vim::ModeIndicator>,
    commands: Entity<crate::editor_command::CommandPrompt>,
    presentation_error: Option<String>,
    semantic_excerpt: Option<Entity<crate::semantic_excerpt::SemanticExcerpt>>,
    /// A zoom into one list item, per pane: the page editor first, then the second.
    zooms: [Option<crate::outline::zoom::Zoom>; 2],
    /// What the stored window shows at the end of the status bar: the page's path and words.
    pub(crate) status: Option<gpui::AnyView>,
}

impl NotebookWindow {
    /// The Write editor, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn write_editor(&self) -> &Entity<Editor> {
        &self.source
    }

    fn tag_summary(&self, cx: &App) -> gpui::SharedString {
        self.analysis
            .read(cx)
            .current_inline(cx)
            .map(|inline| inline.tag_summary.clone())
            .unwrap_or_default()
    }

    fn change_presentation(
        &mut self,
        editor: &Entity<Editor>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.zooms[usize::from(editor == &self.second)] = None;
        self.presentation_error = if crate::editor_projection::enabled(editor.read(cx)) {
            crate::editor_projection::unbind(editor, window, cx);
            None
        } else {
            crate::editor_projection::bind(editor, window, cx).err()
        };
        if editor == &self.second
            && let Some(model) = &self.semantic_excerpt
        {
            crate::semantic_excerpt::refresh_view(editor, model, window, cx);
            if self.presentation_error.is_some() {
                self.presentation_error =
                    model.read(cx).message().or(self.presentation_error.take());
            }
        }
        window.focus(&editor.focus_handle(cx), cx);
        self.refresh_tags(cx);
        cx.notify();
    }

    fn toggle_presentation(
        &mut self,
        _: &TogglePresentation,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(pane) = self.focused_pane(window, cx) else {
            cx.propagate();
            return;
        };
        let editor = self.pane(pane).clone();
        self.change_presentation(&editor, window, cx);
    }

    fn pane(&self, pane: usize) -> &Entity<Editor> {
        if pane == 0 {
            &self.source
        } else {
            &self.second
        }
    }

    /// The pane holding focus, in its page editor or its zoom.
    fn focused_pane(&self, window: &Window, cx: &App) -> Option<usize> {
        (0..2).find(|&pane| {
            std::iter::once(self.pane(pane))
                .chain(self.zooms[pane].as_ref().map(|zoom| &zoom.editor))
                .any(|editor| editor.focus_handle(cx).contains_focused(window, cx))
        })
    }

    /// The editor a pane shows: its zoom, or the page.
    fn shown(&self, pane: usize) -> Entity<Editor> {
        self.zooms[pane]
            .as_ref()
            .map_or_else(|| self.pane(pane).clone(), |zoom| zoom.editor.clone())
    }

    fn zoom_in(
        &mut self,
        _: &crate::outline::ZoomIn,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(pane) = self.focused_pane(window, cx) else {
            cx.propagate();
            return;
        };
        let from = self.shown(pane);
        match crate::outline::zoom::Zoom::open(&from, &self.analysis, window, cx) {
            Ok(zoom) => {
                window.focus(&zoom.editor.focus_handle(cx), cx);
                self.zooms[pane] = Some(zoom);
                crate::outline::set_notice(None, cx);
                self.refresh_tags(cx);
            }
            Err(message) => crate::outline::set_notice(Some(message), cx),
        }
        cx.notify();
    }

    fn zoom_out(
        &mut self,
        _: &crate::outline::ZoomOut,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(pane) = self
            .focused_pane(window, cx)
            .filter(|&pane| self.zooms[pane].is_some())
        else {
            cx.propagate();
            return;
        };
        self.close_zoom(pane, window, cx);
    }

    /// Shows the whole page again with the caret where it was in the zoom.
    fn close_zoom(&mut self, pane: usize, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let Some(zoom) = self.zooms[pane].take() else {
            return;
        };
        let editor = self.pane(pane).clone();
        if let Some(caret) = zoom.caret(cx) {
            editor.update(cx, |editor, cx| {
                editor.change_selections(editor::SelectionEffects::default(), window, cx, |s| {
                    s.select_ranges([multi_buffer::MultiBufferOffset(caret)
                        ..multi_buffer::MultiBufferOffset(caret)])
                });
            });
        }
        window.focus(&editor.focus_handle(cx), cx);
        cx.notify();
    }

    /// The bar above a zoomed pane, with the way back to the whole page.
    fn zoom_bar(&self, pane: usize, cx: &mut gpui::Context<Self>) -> Option<gpui::Div> {
        let zoom = self.zooms[pane].as_ref()?;
        let message = zoom.message(cx);
        Some(
            gpui::div()
                .p_1()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new(("zoom-out", pane), "All blocks")
                        .aria_label("Zoom out to all blocks")
                        .tab_index(0_isize)
                        .tooltip(Tooltip::text("Show the whole page"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_zoom(pane, window, cx)
                        })),
                )
                .child(Label::new("Focused block"))
                .when_some(message, |div, message| div.child(Label::new(message))),
        )
    }

    fn presentation_button(
        &self,
        id: &'static str,
        editor: &Entity<Editor>,
        cx: &mut gpui::Context<Self>,
    ) -> Button {
        let write = crate::editor_projection::enabled(editor.read(cx));
        let label = if write { "Write" } else { "Source" };
        let target = if write {
            "Switch to Source"
        } else {
            "Switch to Write"
        };
        let editor = editor.clone();
        Button::new(id, label)
            .aria_label(format!("{label} view. {target}"))
            .tab_index(0_isize)
            .tooltip(Tooltip::text(target))
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.change_presentation(&editor, window, cx)
                }),
            )
    }

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
    pub(crate) fn second_editor(&self) -> &Entity<Editor> {
        &self.second
    }

    #[cfg(test)]
    pub(crate) fn second_is_written(&self, cx: &App) -> bool {
        crate::editor_projection::enabled(self.second.read(cx))
    }

    #[cfg(test)]
    pub(crate) fn source_text(&self, cx: &App) -> String {
        self.source.read(cx).text(cx)
    }

    #[cfg(test)]
    pub(crate) fn new(text: &str, window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
        let buffer = crate::markdown::buffer(text, cx);
        Self::for_buffer(buffer, None, window, cx)
    }

    #[cfg(test)]
    fn for_buffer(
        buffer: Entity<language::Buffer>,
        excerpt: Option<Entity<multi_buffer::MultiBuffer>>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let analysis = cx.new(|cx| crate::page_analysis::PageAnalysis::new(buffer, cx));
        Self::for_page(analysis, excerpt, window, cx)
    }

    fn refresh_tags(&mut self, cx: &mut gpui::Context<Self>) {
        let zooms = self.zooms.iter().flatten().map(|zoom| &zoom.editor);
        for editor in [&self.source, &self.second].into_iter().chain(zooms) {
            crate::tag_labels::refresh(editor, &self.analysis, cx);
        }
    }

    /// Shows the second pane as Write, the way a reader first meets a page.
    pub(crate) fn show_written(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if !crate::editor_projection::enabled(self.second.read(cx)) {
            let second = self.second.clone();
            self.change_presentation(&second, window, cx);
        }
    }

    pub(crate) fn for_page(
        analysis: Entity<crate::page_analysis::PageAnalysis>,
        excerpt: Option<Entity<multi_buffer::MultiBuffer>>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let buffer = analysis.read(cx).source.clone();
        let mode = cx.new(|cx| vim::ModeIndicator::new(window, cx));
        let source = cx.new(|cx| editor(buffer.clone(), window, cx));
        let second = cx.new(|cx| match excerpt {
            Some(excerpt) => {
                let mut editor = Editor::for_multibuffer(excerpt, None, window, cx);
                crate::page_menu::install(&mut editor);
                editor
            }
            None => editor(buffer, window, cx),
        });
        let source_search = crate::editor_search::new(&source, window, cx);
        let second_search = crate::editor_search::new(&second, window, cx);
        let commands = cx.new(|cx| crate::editor_command::CommandPrompt::new(window, cx));
        cx.observe(&commands, |_, _, cx| cx.notify()).detach();
        cx.observe(&source_search, |_, _, cx| cx.notify()).detach();
        cx.observe(&second_search, |_, _, cx| cx.notify()).detach();
        cx.observe(&analysis, |this, _, cx| {
            this.refresh_tags(cx);
            cx.notify()
        })
        .detach();
        cx.observe_global::<settings::SettingsStore>(|this, cx| this.refresh_tags(cx))
            .detach();
        cx.observe_global::<theme::GlobalTheme>(|this, cx| this.refresh_tags(cx))
            .detach();
        cx.observe_global::<crate::outline::Notice>(|_, cx| cx.notify())
            .detach();
        crate::preferences::follow_system_appearance(window, cx);
        window.focus(&source.focus_handle(cx), cx);
        Self {
            analysis,
            source,
            second,
            source_search,
            second_search,
            commands,
            mode,
            presentation_error: None,
            semantic_excerpt: None,
            zooms: [None, None],
            status: None,
        }
    }

    fn for_block(
        page: Entity<crate::page_analysis::PageAnalysis>,
        id: &str,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let model =
            cx.new(|cx| crate::semantic_excerpt::SemanticExcerpt::new(page.clone(), id.into(), cx));
        let excerpt = model.read(cx).buffer.clone();
        let mut view = Self::for_page(page, Some(excerpt), window, cx);
        crate::semantic_excerpt::attach(&view.second, &model, window, cx);
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        view.semantic_excerpt = Some(model);
        view
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
            .child(self.presentation_button("source-presentation", &self.source, cx))
            .children(self.zoom_bar(0, cx))
            .when(!self.source_search.read(cx).is_dismissed(), |div| {
                div.child(self.source_search.clone())
            })
            .child(
                gpui::div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.shown(0))
                    .children(crate::media::attach::drop_target(&self.source, cx)),
            );
        let message = self
            .semantic_excerpt
            .as_ref()
            .and_then(|model| model.read(cx).message());
        let mut second =
            DivRegistrar::new(|this: &Self, _, _| Some(this.second_search.clone()), cx);
        BufferSearchBar::register(&mut second);
        let second = second
            .into_div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(self.presentation_button("second-presentation", &self.second, cx))
            .children(self.zoom_bar(1, cx))
            .when_some(message, |div, message| {
                div.child(gpui::div().p_2().child(message))
            })
            .when(!self.second_search.read(cx).is_dismissed(), |div| {
                div.child(self.second_search.clone())
            })
            .child(
                gpui::div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.shown(1))
                    .children(crate::media::attach::drop_target(&self.second, cx)),
            );
        gpui::div()
            .size_full()
            .on_action(cx.listener(Self::search_submit))
            .on_action(cx.listener(Self::open_command))
            .on_action(cx.listener(Self::toggle_presentation))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .flex()
            .flex_col()
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
            .when_some(self.presentation_error.as_ref(), |div, error| {
                div.child(gpui::div().p_2().child(error.clone()))
            })
            .when_some(crate::outline::notice(cx), |div, notice| {
                div.child(gpui::div().p_2().child(notice))
            })
            .child(
                gpui::div()
                    .h_10()
                    .p_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(self.mode.clone())
                    .child(
                        gpui::div()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(self.tag_summary(cx)).truncate()),
                    )
                    .children(self.status.clone()),
            )
    }
}

/// Becomes the only Quire for this data folder, or hands this launch to the running one and
/// returns None.
fn claim_instance() -> Result<Option<crate::single_instance::Listener>, String> {
    use crate::single_instance::{Claim, Launch, Reply, claim, socket_path};
    let folder = paths::data_dir();
    std::fs::create_dir_all(folder).map_err(|error| {
        format!(
            "Cannot create Quire's application data folder {}: {error}. Check its permissions, then start Quire again.",
            folder.display()
        )
    })?;
    let launch = Launch {
        version: env!("CARGO_PKG_VERSION").into(),
        executable: std::env::current_exe().unwrap_or_default(),
        args: std::env::args().collect(),
    };
    let socket = socket_path(folder, "quire.sock");
    match claim(&socket, &launch, 50, std::time::Duration::from_millis(200)) {
        Ok(Claim::First(listener)) => Ok(Some(listener)),
        Ok(Claim::Running(Reply::Handover)) => {
            println!("Quire is already running; it will save, quit, and start this newer build.");
            Ok(None)
        }
        Ok(Claim::Running(_)) => {
            println!("Quire is already running; it opened what this launch asked for.");
            Ok(None)
        }
        Err(error) => Err(format!(
            "Cannot reach the running Quire through {}: {error}. Quit the other Quire, then start Quire again.",
            socket.display()
        )),
    }
}

/// Hands later launches to the window thread, and starts a newer build after quitting.
fn serve_launches(listener: crate::single_instance::Listener, cx: &mut App) {
    let quitting = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (sender, mut launches) = futures::channel::mpsc::unbounded();
    if let Err(error) = listener.serve(
        env!("CARGO_PKG_VERSION").into(),
        quitting.clone(),
        move |launch, reply| {
            sender.unbounded_send((launch, reply)).ok();
        },
    ) {
        eprintln!(
            "Cannot listen for other Quire launches: {error}. A second launch may open a second Quire; restart Quire to retry."
        );
    }
    cx.spawn(async move |cx| {
        use futures::StreamExt as _;
        while let Some((launch, reply)) = launches.next().await {
            cx.update(|cx| stored::handle_launch(launch, reply, cx));
        }
    })
    .detach();
    cx.on_app_quit(move |cx| {
        quitting.store(true, std::sync::atomic::Ordering::SeqCst);
        let handover = cx
            .try_global::<stored::Handover>()
            .and_then(|handover| handover.0.clone());
        async move {
            if let Some(launch) = handover
                && let Err(error) = std::process::Command::new(&launch.executable)
                    .args(launch.args.iter().skip(1))
                    .spawn()
            {
                eprintln!(
                    "Cannot start the newer Quire at {}: {error}. Start it again yourself.",
                    launch.executable.display()
                );
            }
        }
    })
    .detach();
}

pub fn run() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = crate::rt_migration::run_from_arguments(&arguments, paths::data_dir()) {
        std::process::exit(code);
    }
    let stored = match stored::OpenOptions::parse(std::env::args().skip(1).collect()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let folder = match stored::FolderOptions::parse(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    if std::env::args().any(|argument| argument == "--input-harness") {
        super::run_example();
        return;
    }
    let excerpt_demo = std::env::args().any(|argument| argument == "--excerpt-harness");
    let write_demo = std::env::args().any(|argument| argument == "--write-harness");
    if excerpt_demo && write_demo {
        eprintln!("Choose one editor harness: pass --write-harness or --excerpt-harness.");
        std::process::exit(2);
    }
    let harness = excerpt_demo || write_demo;
    let listener = if harness {
        None
    } else {
        match claim_instance() {
            Ok(Some(listener)) => Some(listener),
            Ok(None) => return,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    };
    gpui_platform::application().with_assets(crate::quire_assets::Assets).run(move |cx| {
        if let Err(error) = init(cx) {
            eprintln!("{error}");
            cx.quit();
            return;
        }
        let fs = ::fs::RealFs::new(None, cx.background_executor().clone());
        <dyn ::fs::Fs>::set_global(fs.clone(), cx);
        crate::preferences::load(fs, cx);
        crate::profiles::load(paths::data_dir().join("profiles.json"), cx);
        if let Some(line) = crate::rt_migration::import_ai_settings(paths::data_dir(), cx) {
            crate::outline::show_notice(line, cx);
        }
        crate::motion::init(cx);
        crate::app_menus::refresh(cx);
        if let Some(listener) = listener {
            serve_launches(listener, cx);
        }
        if let Err(error) = assets::Assets.load_fonts(cx).and_then(|()| crate::script_fonts::load(cx)) {
            eprintln!("Cannot load Quire's bundled fonts: {error}. Rebuild the application assets.");
            cx.quit();
            return;
        }
        match reqwest_client::ReqwestClient::user_agent(concat!("Quire/", env!("CARGO_PKG_VERSION"))) {
            Ok(client) => cx.set_http_client(std::sync::Arc::new(client)),
            Err(error) => eprintln!("Cannot start the HTTP client: {error:#}. Remote images and PDFs will not load; restart Quire to retry."),
        }
        cx.on_window_closed(|cx, _| { if cx.windows().is_empty() { cx.quit(); } }).detach();
        if let Some(options) = stored {
            if let Err(error) = options.open(cx) {
                eprintln!("{error}");
                cx.quit();
            }
            return;
        }
        if let Some(options) = folder {
            if let Err(error) = options.open(cx) {
                eprintln!("{error}");
                cx.quit();
            }
            return;
        }
        if !harness
            && let Some(notebook) = crate::profiles::get(crate::profiles::active(cx), cx).notebook
        {
            match stored::launch_location(&notebook, cx) {
                Ok(()) => return,
                Err(error) => crate::outline::show_notice(
                    format!("Quire could not reopen the {} notebook: {error}", crate::profiles::active(cx).label()),
                    cx,
                ),
            }
        }
        let prefix = "# Welcome to Quire\n\n";
        let block = "- Edit this note in either pane. ^b-welcome\n  - Both panes share the same document.";
        let text = format!("{prefix}{block}\n- [ ] Try Vim, undo, and redo.\n");
        let buffer = crate::markdown::buffer(&text, cx);
        let page = cx.new(|cx| crate::page_analysis::PageAnalysis::new(buffer, cx));
        let options = gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(None, gpui::size(gpui::px(1100.), gpui::px(760.)), cx))),
            ..Default::default()
        };
        match cx.open_window(options, move |window, cx| {
            window.set_window_title("Quire Native");
            cx.new(|cx| {
                let view = if excerpt_demo {
                    NotebookWindow::for_block(page, "b-welcome", window, cx)
                } else {
                    NotebookWindow::for_page(page, None, window, cx)
                };
                if write_demo
                    && let Err(error) = crate::editor_projection::bind(&view.second, window, cx) {
                        eprintln!("{error}");
                }
                view
            })
        }) {
            Ok(_) => cx.activate(true),
            Err(error) => { eprintln!("Cannot open Quire's window: {error}. Check desktop and graphics access, then retry."); cx.quit(); }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn shared_page_tags_update_both_windows_without_changing_source(cx: &mut TestAppContext) {
        cx.update(|cx| init(cx).unwrap());
        let text = "#one #عربي #one [[#hidden]] `prefix #code`\n";
        let source = cx.update(|cx| crate::markdown::buffer(text, cx));
        let page = cx.new(|cx| crate::page_analysis::PageAnalysis::new(source.clone(), cx));
        let first =
            cx.add_window(|window, cx| NotebookWindow::for_page(page.clone(), None, window, cx));
        let second =
            cx.add_window(|window, cx| NotebookWindow::for_page(page.clone(), None, window, cx));
        cx.executor().run_until_parked();
        for window in [first, second] {
            window
                .update(cx, |view, _, cx| {
                    assert_eq!(view.analysis.entity_id(), page.entity_id());
                    assert_eq!(view.tag_summary(cx), "Inline tags: #one · #عربي");
                })
                .unwrap();
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }
        cx.update(|cx| assert_eq!(source.read(cx).snapshot().text(), text));
        source.update(cx, |source, cx| {
            source.set_sync_parse_timeout(None);
            source.edit([(0..source.len(), "#new #new\n")], None, cx);
        });
        for window in [first, second] {
            window
                .update(cx, |view, _, cx| assert!(view.tag_summary(cx).is_empty()))
                .unwrap();
        }
        cx.executor().run_until_parked();
        for window in [first, second] {
            window
                .update(cx, |view, _, cx| {
                    assert_eq!(view.tag_summary(cx), "Inline tags: #new")
                })
                .unwrap();
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }
        cx.update(|cx| assert_eq!(source.read(cx).snapshot().text(), "#new #new\n"));
    }

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

#[cfg(test)]
#[path = "editor_projection_tests.rs"]
mod projection_tests;

#[cfg(test)]
#[path = "editor_projection_vim_tests.rs"]
mod projection_vim_tests;

#[cfg(test)]
#[path = "editor_link_tests.rs"]
mod link_tests;

#[cfg(test)]
#[path = "editor_projection_mapping_tests.rs"]
mod projection_mapping_tests;

#[cfg(test)]
#[path = "semantic_excerpt_tests.rs"]
mod semantic_excerpt_tests;

#[cfg(test)]
#[path = "outline/zoom_tests.rs"]
mod zoom_tests;
