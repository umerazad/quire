use gpui::{Empty, TestAppContext};
use std::{cell::Cell, rc::Rc, time::Duration};

#[gpui::test]
fn quit_without_guards_keeps_the_original_programmatic_and_native_behavior(
    cx: &mut TestAppContext,
) {
    assert!(cx.simulate_native_quit_request());
    cx.update(|cx| cx.quit());
    assert_eq!(cx.platform_quit_count(), 1);
}

#[gpui::test]
fn veto_preserves_windows_and_shutdown_handlers_until_a_later_native_allow(
    cx: &mut TestAppContext,
) {
    let allowed = Rc::new(Cell::new(false));
    let shutdown = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, _| Empty);
    let guard = cx.update(|cx| {
        cx.on_request_quit({
            let allowed = allowed.clone();
            move |cx| {
                assert_eq!(cx.windows().len(), 1);
                allowed.get()
            }
        })
    });
    cx.update(|cx| {
        cx.on_app_quit({
            let shutdown = shutdown.clone();
            move |_| {
                let shutdown = shutdown.clone();
                async move {
                    shutdown.set(shutdown.get() + 1);
                }
            }
        })
        .detach()
    });
    cx.update(|cx| cx.quit());
    assert_eq!(cx.platform_quit_count(), 0);
    assert!(!cx.simulate_native_quit_request());
    assert_eq!(shutdown.get(), 0);
    window.update(cx, |_, _, _| {}).unwrap();
    allowed.set(true);
    assert!(cx.simulate_native_quit_request());
    assert_eq!(shutdown.get(), 0);
    drop(guard);
    cx.quit();
    assert_eq!(shutdown.get(), 1);
}

#[gpui::test]
fn every_guard_is_consulted_and_dropping_a_subscription_removes_its_veto(cx: &mut TestAppContext) {
    let first_calls = Rc::new(Cell::new(0));
    let second_calls = Rc::new(Cell::new(0));
    let first = cx.update(|cx| {
        cx.on_request_quit({
            let calls = first_calls.clone();
            move |_| {
                calls.set(calls.get() + 1);
                false
            }
        })
    });
    let second = cx.update(|cx| {
        cx.on_request_quit({
            let calls = second_calls.clone();
            move |_| {
                calls.set(calls.get() + 1);
                true
            }
        })
    });
    cx.update(|cx| cx.quit());
    assert_eq!(first_calls.get(), 1);
    assert_eq!(second_calls.get(), 1);
    assert_eq!(cx.platform_quit_count(), 0);
    drop(first);
    cx.update(|cx| cx.quit());
    assert_eq!(first_calls.get(), 1);
    assert_eq!(second_calls.get(), 2);
    assert_eq!(cx.platform_quit_count(), 1);
    drop(second);
    assert!(cx.simulate_native_quit_request());
}

#[gpui::test]
fn asynchronous_preparation_can_outlive_the_shutdown_grace_without_quitting(
    cx: &mut TestAppContext,
) {
    let ready = Rc::new(Cell::new(false));
    let pending = Rc::new(Cell::new(false));
    let guard = cx.update(|cx| {
        cx.on_request_quit({
            let ready = ready.clone();
            let pending = pending.clone();
            move |cx| {
                if ready.get() {
                    return true;
                }
                if !pending.replace(true) {
                    let delay = cx.background_executor().timer(Duration::from_secs(10));
                    let ready = ready.clone();
                    cx.spawn(async move |cx| {
                        delay.await;
                        ready.set(true);
                        cx.update(|cx| cx.quit());
                    })
                    .detach();
                }
                false
            }
        })
    });
    cx.executor().allow_parking();
    cx.update(|cx| cx.quit());
    while cx.executor().tick() {}
    assert_eq!(cx.platform_quit_count(), 0);
    cx.executor().advance_clock(Duration::from_secs(1));
    while cx.executor().tick() {}
    assert_eq!(cx.platform_quit_count(), 0);
    assert!(!cx.simulate_native_quit_request());
    cx.executor().advance_clock(Duration::from_secs(9));
    while cx.executor().tick() {}
    assert_eq!(cx.platform_quit_count(), 1);
    assert!(ready.get());
    drop(guard);
}

#[gpui::test]
fn reentrant_requests_cannot_escape_an_active_veto(cx: &mut TestAppContext) {
    let calls = Rc::new(Cell::new(0));
    let guard = cx.update(|cx| {
        cx.on_request_quit({
            let calls = calls.clone();
            move |cx| {
                calls.set(calls.get() + 1);
                cx.quit();
                false
            }
        })
    });
    cx.update(|cx| cx.quit());
    assert_eq!(calls.get(), 1);
    assert_eq!(cx.platform_quit_count(), 0);
    assert!(!cx.simulate_native_quit_request());
    assert_eq!(calls.get(), 2);
    drop(guard);
}
