//! maxly-gpui: a telemetry-free MAX desktop client (ADR-007: gpui + gpui-kit).
//!
//! `maxly-gpui --demo` shows the screens on made-up data without any network access;
//! `maxly-gpui --demo-walk` steps through every screen on a timer (for screenshots);
//! `maxly-gpui --trace` writes a local protocol trace without personal data (`trace.rs`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod album;
mod app_view;
mod backend;
mod desktop;
mod instance;
mod qr;
mod rich;
mod settings;
mod trace;
mod viewer;

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::*;

use crate::app_view::AppView;
use crate::backend::Backend;

fn main() {
    let walk = std::env::args().any(|a| a == "--demo-walk");
    let demo = walk || std::env::args().any(|a| a == "--demo");
    if !demo && std::env::args().any(|a| a == "--trace") {
        match trace::enable() {
            Ok(path) => eprintln!("protocol trace: {}", path.display()),
            Err(e) => eprintln!("cannot start the protocol trace: {e}"),
        }
    }
    // One client per user: a second start raises the running one and leaves before it connects.
    // The demo skips the check, so it can run next to a real client.
    let instance = if demo {
        instance::Claim::Alone
    } else {
        instance::claim()
    };
    let instance = match instance {
        instance::Claim::Running => return,
        instance::Claim::First(i) => Some(i),
        instance::Claim::Alone => None,
    };
    let backend = match Backend::start(demo, walk) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot start the client runtime: {e}");
            std::process::exit(1);
        }
    };
    let Backend {
        _runtime: runtime,
        events,
        commands,
    } = backend;

    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            // The AppUserModelID Windows shows notifications under (registered for this user on
            // the first notification, as GPUI does for unpackaged apps).
            cx.set_app_identity("maxly-gpui.desktop", "maxly-gpui");
            let title: SharedString = if demo {
                "maxly-gpui — демо"
            } else {
                "maxly-gpui"
            }
            .into();
            // GPUI otherwise holds an inactive window and Low Power Mode at about 30 fps,
            // and a hot machine at about 60. 120 Hz is one frame every 8.3 ms; a slower
            // display still waits for its own vsync, so this does not draw faster than the screen.
            let frame = std::time::Duration::from_micros(8_333);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1100.), px(720.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.clone()),
                    ..Default::default()
                }),
                inactive_frame_interval: Some(frame),
                low_power_frame_interval: Some(frame),
                thermal_frame_interval: Some(frame),
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| AppView::new(events, commands, instance, title, window, cx))
            });
            if let Err(e) = opened {
                eprintln!("cannot open the main window: {e}");
                cx.quit();
            }
        });
    drop(runtime);
}

// Lucide icons beyond the component set: the navigation rail, the settings pages, the message
// menu and the photo viewer.
icon_assets!(
    ExtraIcons,
    [
        MessageCircle,
        Phone,
        Contact,
        Laptop,
        Reply,
        Pencil,
        Trash,
        ZoomIn,
        ZoomOut,
        Download,
        X
    ]
);

/// The component icons plus [`ExtraIcons`].
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
