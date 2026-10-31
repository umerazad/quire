//! Reduced motion follows the system setting, as the reference app follows
//! `prefers-reduced-motion`: GPUI's animations then draw their last frame, the editor's cursor
//! glides stop, and the cursor no longer blinks. A change to the setting applies at once.

use gpui::App;

/// Applies the system's reduced motion setting now and whenever it changes.
pub fn init(cx: &mut App) {
    cx.set_reduce_motion(system::reduce_motion());
    system::watch(cx);
}

#[cfg(target_os = "macos")]
mod system {
    use futures::StreamExt as _;
    use gpui::App;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};

    pub fn reduce_motion() -> bool {
        NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }

    pub fn watch(cx: &mut App) {
        let (changed, mut changes) = futures::channel::mpsc::unbounded::<()>();
        let block = block2::RcBlock::new(move |_| {
            changed.unbounded_send(()).ok();
        });
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: the notification has no object, and the block only sends on a channel, which is
        // safe from whichever thread posts the notification.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
                None,
                &block,
            )
        };
        cx.spawn(async move |cx| {
            let _observer = observer;
            while changes.next().await.is_some() {
                cx.update(|cx| cx.set_reduce_motion(reduce_motion()));
            }
        })
        .detach();
    }
}

#[cfg(not(target_os = "macos"))]
mod system {
    pub fn reduce_motion() -> bool {
        false
    }

    pub fn watch(_: &mut gpui::App) {}
}

#[cfg(test)]
mod tests {
    use editor::Editor;
    use gpui::{AppContext as _, Focusable as _, TestAppContext};
    use std::time::Duration;

    const BLINK: Duration = Duration::from_millis(500);

    fn cursor_shown_through_blinks(reduce: bool, cx: &mut TestAppContext) -> Vec<bool> {
        cx.update(|cx| cx.set_reduce_motion(reduce));
        let window = cx.add_window(Editor::single_line);
        window
            .update(cx, |editor, window, cx| {
                window.focus(&editor.focus_handle(cx), cx);
                window.activate_window();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.run_until_parked();
        (0..4)
            .map(|_| {
                cx.executor().advance_clock(BLINK);
                cx.run_until_parked();
                window
                    .update(cx, |editor, window, cx| {
                        editor.show_local_cursors(window, cx)
                    })
                    .unwrap()
            })
            .collect()
    }

    #[gpui::test]
    fn the_cursor_stops_blinking_under_reduced_motion(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        assert!(
            cursor_shown_through_blinks(false, cx).contains(&false),
            "without reduced motion the cursor blinks"
        );
        assert_eq!(cursor_shown_through_blinks(true, cx), [true; 4]);
    }
}
