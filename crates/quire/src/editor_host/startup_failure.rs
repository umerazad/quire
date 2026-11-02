//! Startup failures shown in a dialog rather than only on standard error, as the reference shows
//! them: what failed and what to do next, with Try Again and Open Another Folder when a notebook
//! did not open, and Quit.

use gpui::{
    App, AppContext as _, Bounds, PromptLevel, Window, WindowBounds, WindowHandle, WindowOptions,
    div, prelude::*, px, size,
};
use std::rc::Rc;

/// Opens the notebook again, as the launch that failed tried to.
pub type Retry = Rc<dyn Fn(&mut App) -> Result<(), String>>;

pub const TITLE: &str = "Quire could not start";
pub const TRY_AGAIN: &str = "Try Again";
pub const OPEN_ANOTHER: &str = "Open Another Folder…";
pub const QUIT: &str = "Quit";

/// The window the dialog sits on. It draws nothing itself, so it shows before the theme and
/// fonts load.
pub struct StartupFailure;

impl Render for StartupFailure {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

/// The dialog's buttons: retrying needs the notebook to open, and opening another folder needs
/// Quire's commands, which a failure before they load leaves out.
pub fn answers(retry: bool) -> &'static [&'static str] {
    match retry {
        true => &[TRY_AGAIN, OPEN_ANOTHER, QUIT],
        false => &[QUIT],
    }
}

/// Shows `message` in a dialog and also prints it, for a launch from a terminal. With `retry`,
/// Try Again runs it and Open Another Folder asks for a notebook folder; Quit, or closing the
/// dialog, quits.
pub fn show(message: String, retry: Option<Retry>, cx: &mut App) {
    eprintln!("{message}");
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(520.), px(200.)),
            cx,
        ))),
        ..crate::title_bar::base_options()
    };
    match cx.open_window(options, |window, cx| {
        window.set_window_title("Quire");
        cx.new(|_| StartupFailure)
    }) {
        Ok(handle) => ask(handle, message, retry, cx),
        Err(error) => {
            eprintln!(
                "Cannot show the startup failure in a window: {error}. Read the message above, then restart Quire."
            );
            cx.quit();
        }
    }
}

fn ask(handle: WindowHandle<StartupFailure>, message: String, retry: Option<Retry>, cx: &mut App) {
    let answer = handle.update(cx, |_, window, cx| {
        window.prompt(
            PromptLevel::Critical,
            TITLE,
            Some(&message),
            answers(retry.is_some()),
            cx,
        )
    });
    let Ok(answer) = answer else {
        cx.quit();
        return;
    };
    cx.spawn(async move |cx| {
        let answer = answer.await.ok();
        cx.update(|cx| match (answer, retry) {
            (Some(0), Some(retry)) => match retry(cx) {
                Ok(()) => close(handle, cx),
                Err(error) => ask(handle, error, Some(retry), cx),
            },
            (Some(1), Some(retry)) => open_another(handle, message, retry, cx),
            _ => cx.quit(),
        });
    })
    .detach();
}

fn close(handle: WindowHandle<StartupFailure>, cx: &mut App) {
    handle
        .update(cx, |_, window, _| window.remove_window())
        .ok();
}

/// Asks for a notebook folder and opens it in the active profile, as File > Open Notebook
/// Folder does. The dialog closes once a notebook page shows; a cancel, or a folder that cannot
/// open, asks again.
fn open_another(handle: WindowHandle<StartupFailure>, message: String, retry: Retry, cx: &mut App) {
    let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Open".into()),
    });
    cx.spawn(async move |cx| {
        let root = chosen
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .and_then(|paths| paths.into_iter().next());
        cx.update(|cx| {
            let Some(root) = root else {
                return ask(handle, message, Some(retry), cx);
            };
            cx.observe_new::<super::NotebookWindow>(move |_, _, cx| {
                cx.defer(move |cx| close(handle, cx));
            })
            .detach();
            let before = crate::outline::notice(cx);
            let id = crate::profiles::active(cx);
            super::stored::open_chosen_folder(id, root, paths::data_dir().clone(), cx);
            if let Some(refused) =
                crate::outline::notice(cx).filter(|now| Some(now) != before.as_ref())
            {
                ask(handle, refused.to_string(), Some(retry), cx);
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::cell::Cell;

    fn failure_windows(cx: &mut TestAppContext) -> usize {
        cx.update(|cx| {
            cx.windows()
                .into_iter()
                .filter(|window| window.downcast::<StartupFailure>().is_some())
                .count()
        })
    }

    #[gpui::test]
    fn a_notebook_that_did_not_open_is_shown_and_tried_again(cx: &mut TestAppContext) {
        let tries = Rc::new(Cell::new(0));
        let retry: Retry = Rc::new({
            let tries = tries.clone();
            move |cx| {
                tries.set(tries.get() + 1);
                match tries.get() {
                    1 => Err("Still locked. Close the other Quire, then try again.".into()),
                    // A retry that works opens the notebook's window, as the real one does;
                    // without it, closing the dialog off macOS would close the last window and
                    // quit.
                    _ => cx
                        .open_window(WindowOptions::default(), |_, cx| cx.new(|_| gpui::Empty))
                        .map(drop)
                        .map_err(|error| error.to_string()),
                }
            }
        });
        cx.update(|cx| {
            show(
                "The notebook is locked. Close the other Quire, then try again.".into(),
                Some(retry),
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(
            cx.pending_prompt(),
            Some((
                TITLE.to_owned(),
                "The notebook is locked. Close the other Quire, then try again.".to_owned()
            ))
        );
        cx.simulate_prompt_answer(OPEN_ANOTHER);
        cx.run_until_parked();
        assert!(
            cx.did_prompt_for_paths(),
            "Open Another Folder asks for one"
        );
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
        assert_eq!(
            cx.pending_prompt().map(|(_, detail)| detail),
            Some("The notebook is locked. Close the other Quire, then try again.".to_owned()),
            "a cancelled choice asks again"
        );
        cx.simulate_prompt_answer(TRY_AGAIN);
        cx.run_until_parked();
        assert_eq!(
            cx.pending_prompt().map(|(_, detail)| detail),
            Some("Still locked. Close the other Quire, then try again.".to_owned()),
            "a failed retry shows its own failure"
        );
        cx.simulate_prompt_answer(TRY_AGAIN);
        cx.run_until_parked();
        assert_eq!(tries.get(), 2);
        assert!(!cx.has_pending_prompt());
        assert_eq!(
            failure_windows(cx),
            0,
            "the dialog goes once the notebook opens"
        );
        assert_eq!(cx.platform_quit_count(), 0);
    }

    #[gpui::test]
    fn a_failure_with_nothing_to_retry_offers_only_quit(cx: &mut TestAppContext) {
        assert_eq!(answers(false), [QUIT]);
        cx.update(|cx| {
            show(
                "Cannot load Quire's bundled fonts: missing. Reinstall Quire.".into(),
                None,
                cx,
            )
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer(QUIT);
        cx.run_until_parked();
        assert_eq!(cx.platform_quit_count(), 1);
    }
}
