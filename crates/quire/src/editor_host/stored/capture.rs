//! `quire --capture-shell --notebook <folder> --out <png> [--width 850] [--height 1769]
//! [--appearance light|dark] [--page <relative path>]`, built with `test-support`: renders a folder
//! notebook's window to a PNG without showing it, for comparing the window against reference
//! screenshots. The notebook is copied into a fresh temporary folder that also holds Quire's
//! application data, so neither the given folder nor the user's own Quire data is read or
//! written. It waits for the page, the index, and the page list before drawing.

use super::{FolderOptions, StoredWindow};
use gpui::{AnyWindowHandle, App, HeadlessAppContext, WindowAppearance, WindowHandle, px, size};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use theme::{Appearance, SystemAppearance};

const USAGE: &str = "Usage: quire --capture-shell --notebook <folder> --out <png> [--width 850] [--height 1769] [--appearance light|dark] [--page <relative path>]";
/// How long background storage and index work may take before the capture gives up.
const PATIENCE: Duration = Duration::from_secs(60);

struct Request {
    notebook: PathBuf,
    out: PathBuf,
    width: f32,
    height: f32,
    dark: bool,
    page: Option<String>,
}

impl Request {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let value = |flag: &str| {
            arguments
                .iter()
                .position(|argument| argument == flag)
                .map(|index| {
                    arguments
                        .get(index + 1)
                        .cloned()
                        .ok_or_else(|| format!("Missing value for {flag}. {USAGE}"))
                })
                .transpose()
        };
        let number = |flag: &str, default: f32| -> Result<f32, String> {
            value(flag)?.map_or(Ok(default), |text| {
                text.parse()
                    .ok()
                    .filter(|number: &f32| (100. ..=8000.).contains(number))
                    .ok_or_else(|| format!("{flag} takes a size from 100 to 8000 pixels. {USAGE}"))
            })
        };
        let dark = match value("--appearance")?.as_deref() {
            None | Some("dark") => true,
            Some("light") => false,
            Some(other) => {
                return Err(format!(
                    "Unknown appearance {other}. Pass light or dark. {USAGE}"
                ));
            }
        };
        Ok(Self {
            notebook: value("--notebook")?
                .ok_or_else(|| format!("Missing --notebook. {USAGE}"))?
                .into(),
            out: value("--out")?
                .ok_or_else(|| format!("Missing --out. {USAGE}"))?
                .into(),
            width: number("--width", 850.)?,
            height: number("--height", 1769.)?,
            dark,
            page: value("--page")?,
        })
    }
}

fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_folder(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Runs `step` until `done` holds, sleeping a little real time between looks so storage and
/// index threads can answer.
fn wait(
    cx: &mut HeadlessAppContext,
    what: &str,
    mut done: impl FnMut(&mut HeadlessAppContext) -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        cx.run_until_parked();
        cx.advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        if done(cx) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Timed out waiting for {what}. Check the notebook opens in Quire, then retry."
            ));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn read<T>(
    cx: &mut HeadlessAppContext,
    window: WindowHandle<StoredWindow>,
    f: impl FnOnce(&StoredWindow, &App) -> T,
) -> Result<T, String> {
    window
        .read_with(cx, f)
        .map_err(|error| format!("The notebook window closed while capturing: {error}. Retry."))
}

fn settle(cx: &mut HeadlessAppContext, window: WindowHandle<StoredWindow>) -> Result<(), String> {
    wait(cx, "the page to load", |cx| {
        read(cx, window, |view, cx| {
            !view.nav.daily.open_on_launch && view.page.read(cx).loaded().is_some()
        })
        .unwrap_or(true)
    })?;
    wait(cx, "the index to start", |cx| {
        read(cx, window, |view, cx| view.index_client(cx).is_some()).unwrap_or(true)
    })?;
    let client = read(cx, window, |view, cx| view.index_client(cx))?
        .ok_or("The index stopped while capturing. Retry.")?;
    let unanalyzed = |client: &quire_store::index::IndexClient| {
        client
            .unanalyzed()
            .ok()
            .and_then(|pages| futures::executor::block_on(pages).ok())
    };
    wait(cx, "indexing", |_| unanalyzed(&client) == Some(0))?;
    let generation = client
        .with_generation(|_| Ok(()))
        .ok()
        .and_then(|answer| futures::executor::block_on(answer).ok())
        .map(|(generation, ())| generation)
        .ok_or("Cannot read the index generation. Retry.")?;
    wait(cx, "the index sync to announce the analyzed pages", |cx| {
        read(cx, window, |view, cx| {
            view.index
                .as_ref()
                .is_ok_and(|index| index.read(cx).announced_generation() >= generation)
        })
        .unwrap_or(true)
    })?;
    window
        .update(cx, |view, _, cx| view.refresh_catalogue(cx))
        .map_err(|error| format!("Cannot refresh the page list: {error}. Retry."))?;
    wait(cx, "the page list", |cx| {
        read(cx, window, |view, _| view.nav.catalogue.is_none()).unwrap_or(true)
    })
}

fn draw(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Result<(), String> {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .map_err(|error| format!("Cannot draw the notebook window: {error}. Retry."))?;
    cx.run_until_parked();
    Ok(())
}

fn capture(request: Request, scratch: &Path) -> Result<(), String> {
    // Keeps the folder name, which the window title and breadcrumb show.
    let notebook = scratch.join(request.notebook.file_name().unwrap_or("notebook".as_ref()));
    copy_folder(&request.notebook, &notebook).map_err(|error| {
        format!(
            "Cannot copy the notebook {}: {error}. Pass an existing, readable folder to --notebook.",
            request.notebook.display()
        )
    })?;
    let platform = gpui_platform::current_platform(true);
    let mut cx = HeadlessAppContext::with_platform(
        platform.text_system(),
        Arc::new(crate::quire_assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.allow_parking();
    let appearance = if request.dark {
        Appearance::Dark
    } else {
        Appearance::Light
    };
    cx.update(|cx| -> Result<(), String> {
        super::super::init(cx)?;
        let fs = ::fs::RealFs::new(None, cx.background_executor().clone());
        <dyn ::fs::Fs>::set_global(fs, cx);
        crate::profiles::load(paths::data_dir().join("profiles.json"), cx);
        assets::Assets
            .load_fonts(cx)
            .and_then(|()| crate::script_fonts::load(cx))
            .map_err(|error| {
                format!(
                    "Cannot load Quire's bundled fonts: {error}. Rebuild the application assets."
                )
            })?;
        *SystemAppearance::global_mut(cx) = SystemAppearance(appearance);
        theme_settings::reload_theme(cx);
        let options = FolderOptions::new(notebook.clone(), Vec::new(), None);
        match &request.page {
            Some(page) => options.on_page(page),
            None => options.on_today(),
        }
        .open(cx)
    })?;
    let window = cx
        .update(|cx| {
            cx.windows()
                .into_iter()
                .find_map(|window| window.downcast::<StoredWindow>())
        })
        .ok_or("The notebook window did not open. Check the notebook folder, then retry.")?;
    cx.simulate_window_appearance(
        window.into(),
        if request.dark {
            WindowAppearance::Dark
        } else {
            WindowAppearance::Light
        },
    );
    cx.simulate_window_resize(window.into(), size(px(request.width), px(request.height)));
    settle(&mut cx, window)?;
    draw(&mut cx, window.into())?;
    draw(&mut cx, window.into())?;
    let image = cx.capture_screenshot(window.into()).map_err(|error| {
        format!("Cannot render the window: {error:#}. Check Metal or GPU access, then retry.")
    })?;
    image.save(&request.out).map_err(|error| {
        format!(
            "Cannot write {}: {error}. Pass a writable .png path to --out.",
            request.out.display()
        )
    })
}

/// Runs the capture and exits with a nonzero status on failure.
pub fn run(arguments: Vec<String>) {
    let request = match Request::parse(&arguments) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let scratch = std::env::temp_dir().join(format!("quire-capture-{}", uuid::Uuid::new_v4()));
    paths::set_custom_data_dir(&scratch.join("data").to_string_lossy());
    let result = capture(request, &scratch);
    std::fs::remove_dir_all(&scratch).ok();
    match result {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Request;

    fn parse(arguments: &[&str]) -> Result<Request, String> {
        Request::parse(
            &arguments
                .iter()
                .map(|argument| argument.to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn capture_requests_default_to_a_dark_850_by_1769_window_on_today() {
        let request = parse(&["--notebook", "nb", "--out", "shot.png"]).unwrap();
        assert_eq!(request.notebook, std::path::PathBuf::from("nb"));
        assert_eq!(request.out, std::path::PathBuf::from("shot.png"));
        assert_eq!((request.width, request.height), (850., 1769.));
        assert!(request.dark);
        assert_eq!(request.page, None);
        let light = parse(&[
            "--notebook",
            "nb",
            "--out",
            "a.png",
            "--appearance",
            "light",
            "--width",
            "1067",
            "--page",
            "Welcome.md",
        ])
        .unwrap();
        assert!(!light.dark);
        assert_eq!(light.width, 1067.);
        assert_eq!(light.page.as_deref(), Some("Welcome.md"));
    }

    #[test]
    fn capture_requests_name_what_is_missing_or_wrong() {
        let error = |arguments: &[&str]| parse(arguments).err().unwrap();
        assert!(error(&["--out", "a.png"]).starts_with("Missing --notebook."));
        assert!(error(&["--notebook", "nb"]).starts_with("Missing --out."));
        assert!(
            error(&[
                "--notebook",
                "nb",
                "--out",
                "a.png",
                "--appearance",
                "sepia"
            ])
            .starts_with("Unknown appearance sepia.")
        );
        assert!(
            error(&["--notebook", "nb", "--out", "a.png", "--width", "9"])
                .starts_with("--width takes a size from 100 to 8000 pixels.")
        );
        assert!(error(&["--notebook", "nb", "--out"]).starts_with("Missing value for --out."));
    }
}
