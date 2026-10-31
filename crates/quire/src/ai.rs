//! AI through the Claude Code and Codex command line tools: the process host, the backend
//! settings, the prompt, and the proposal shown under the text until it is accepted (NR28); the
//! answer pane for questions and syntheses, keeping answers in today's page, and Tidy (NR30).

pub mod answer;
pub mod context;
pub mod keep;
pub mod process;
pub mod prompt;
pub mod proposal;
pub mod settings;

use editor::Editor;
use gpui::{App, Context, Entity, KeyBinding, Window, actions};

actions!(
    quire_ai,
    [
        /// Opens the AI prompt under the block, selection, or blank line at the caret.
        AskAi,
        /// Opens the AI prompt for the lines of the Vim Visual selection, leaving Visual mode.
        AskAiOnVisualLines,
        /// Accepts the finished AI proposal under the text.
        AcceptProposal,
        /// Rejects the AI proposal, or cancels the run still writing it.
        RejectProposal,
        /// Opens the answer pane to ask the notebook a question.
        AskNotebook,
        /// Proposes links, task states, and due dates for the daily page, for review.
        TidyDay,
        /// Sends the question typed in the answer pane.
        SubmitQuestion,
        /// Runs the synthesis instruction typed in the answer pane.
        RunSynthesis,
    ]
);

/// Runs `act` on the editor when it receives `A`.
fn register<A: gpui::Action>(
    editor: &mut Editor,
    cx: &mut Context<Editor>,
    act: impl Fn(&Entity<Editor>, &mut Window, &mut App) + 'static,
) {
    let handle = cx.entity().downgrade();
    editor
        .register_action(move |_: &A, window, cx| {
            if let Some(editor) = handle.upgrade() {
                act(&editor, window, cx);
            }
        })
        .detach();
}

/// The 0-based buffer rows the newest selection touches, when it is not empty.
fn selected_rows(editor: &Entity<Editor>, cx: &App) -> Option<std::ops::Range<u32>> {
    let editor = editor.read(cx);
    let selection = editor.selections.newest_anchor();
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let start = multi_buffer::ToPoint::to_point(&selection.start, &snapshot);
    let end = multi_buffer::ToPoint::to_point(&selection.end, &snapshot);
    (start != end).then(|| start.min(end).row..start.max(end).row)
}

/// Records the Visual selection's lines, leaves Visual mode, then opens the prompt for them, as
/// the reference `\a` does.
fn ask_on_visual_lines(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) {
    let rows = selected_rows(editor, cx);
    window.dispatch_action(Box::new(vim::SwitchToNormalMode), cx);
    let editor = editor.clone();
    window.defer(cx, move |window, cx| {
        prompt::open(&editor, rows, window, cx)
    });
}

pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx| {
        register::<AskAi>(editor, cx, |editor, window, cx| {
            prompt::open(editor, None, window, cx)
        });
        register::<AskAiOnVisualLines>(editor, cx, ask_on_visual_lines);
        register::<AskNotebook>(editor, cx, |editor, window, cx| {
            proposal::start(
                editor,
                proposal::Invocation {
                    args: String::new(),
                    rows: None,
                    verb: None,
                },
                window,
                cx,
            )
        });
        register::<TidyDay>(editor, cx, proposal::tidy);
        register::<AcceptProposal>(editor, cx, |editor, window, cx| {
            if !proposal::accept_key(editor, window, cx) {
                cx.propagate();
            }
        });
        register::<RejectProposal>(editor, cx, |editor, window, cx| {
            if !proposal::reject(editor, window, cx) {
                cx.propagate();
            }
        });
    })
    .detach();
}

/// Binds the AI keys after Vim's own, so they win in the page editors: Command-I (Ctrl-I
/// elsewhere) and Vim's `\a` open the prompt; Enter accepts and Escape rejects a proposal
/// outside Insert mode.
pub fn bind_keys(cx: &mut App) {
    let proposal = "Editor && QuireProposal && !QuireControl && vim_mode != insert";
    cx.bind_keys([
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-i"
            } else {
                "ctrl-i"
            },
            AskAi,
            Some(crate::editor_host::EDITOR_CONTEXT),
        ),
        KeyBinding::new(
            "\\ a",
            AskAi,
            Some("Editor && VimControl && vim_mode == normal && !QuireControl"),
        ),
        KeyBinding::new(
            "\\ a",
            AskAiOnVisualLines,
            Some("Editor && VimControl && vim_mode == visual && !QuireControl"),
        ),
        KeyBinding::new("enter", AcceptProposal, Some(proposal)),
        KeyBinding::new("escape", RejectProposal, Some(proposal)),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-alt-i"
            } else {
                "ctrl-alt-i"
            },
            AskNotebook,
            Some(crate::editor_host::EDITOR_CONTEXT),
        ),
        KeyBinding::new(
            "\\ q",
            AskNotebook,
            Some("Editor && VimControl && vim_mode == normal && !QuireControl"),
        ),
        KeyBinding::new("enter", SubmitQuestion, Some("QuireAnswerInput > Editor")),
        KeyBinding::new(
            "shift-enter",
            editor::actions::Newline,
            Some("QuireAnswerInput > Editor"),
        ),
        KeyBinding::new(
            "enter",
            editor::actions::Newline,
            Some("QuireSynthesis > Editor"),
        ),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-enter"
            } else {
                "ctrl-enter"
            },
            RunSynthesis,
            Some("QuireSynthesis > Editor"),
        ),
    ]);
}
