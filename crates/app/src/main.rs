//! maxrs: a telemetry-free MAX desktop client (ADR-007: gpui + gpui-kit).
//!
//! `maxrs --demo` shows the screens on made-up data without any network access;
//! `maxrs --demo-walk` steps through every screen on a timer (for screenshots);
//! `maxrs --trace` writes a local protocol trace without personal data (`trace.rs`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_view;
mod backend;
mod qr;
mod settings;
mod trace;

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
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1100.), px(720.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some(if demo {
                        "maxrs — демо".into()
                    } else {
                        "maxrs".into()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| AppView::new(events, commands, window, cx))
            });
            if let Err(e) = opened {
                eprintln!("cannot open the main window: {e}");
                cx.quit();
            }
        });
    drop(runtime);
}

// Lucide icons beyond the component set: the navigation rail and the settings pages.
icon_assets!(ExtraIcons, [MessageCircle, Phone, Contact, Laptop]);

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
