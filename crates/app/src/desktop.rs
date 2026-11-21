//! The tray icon, and showing or concealing the window.
//!
//! Windows hides the HWND through `winsafe` (GPUI cannot hide a window there). Linux minimizes
//! and raises the GPUI window. The tray on Linux speaks the StatusNotifier protocol (`ksni`),
//! which KDE, waybar and swaybar provide; GNOME needs the AppIndicator extension. Arch does
//! not need `libappindicator`. No unsafe code here.

/// What the user asked for through the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopEvent {
    /// Show and raise the window.
    Show,
    /// Quit the application.
    Quit,
}

/// Side of the square tray icon, px.
pub const ICON_SIZE: u32 = 32;

/// The tray icon as RGBA pixels: a blue disc with a white speech bubble, and a red dot in the
/// top right corner while there are unread messages.
pub fn tray_pixels(unread: bool) -> Vec<u8> {
    let n = ICON_SIZE as f32;
    let mut out = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let disc = (fx - n / 2.).hypot(fy - n / 2.) <= n / 2. - 0.5;
            // The bubble: an ellipse with a small tail at the bottom left.
            let (ex, ey) = ((fx - n / 2.) / (n * 0.27), (fy - n * 0.46) / (n * 0.21));
            let tail = fx > n * 0.3
                && fx < n * 0.42
                && fy > n * 0.6
                && fy < n * 0.76
                && (fy - n * 0.6) < (n * 0.42 - fx) * 1.4;
            let bubble = ex * ex + ey * ey <= 1. || tail;
            let dot = unread && (fx - n * 0.8).hypot(fy - n * 0.2) <= n * 0.2;
            let rgba = if dot {
                [0xE5, 0x39, 0x35, 0xFF]
            } else if disc && bubble {
                [0xFF, 0xFF, 0xFF, 0xFF]
            } else if disc {
                [0x2F, 0x7C, 0xF6, 0xFF]
            } else {
                [0, 0, 0, 0]
            };
            out.extend_from_slice(&rgba);
        }
    }
    out
}

/// The tray tooltip for `unread` messages.
pub fn tooltip(unread: i32) -> String {
    if unread > 0 {
        format!("maxly-gpui — непрочитанных: {unread}")
    } else {
        "maxly-gpui".to_string()
    }
}

#[cfg(any(windows, target_os = "linux"))]
mod tray {
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    use super::{DesktopEvent, ICON_SIZE, tooltip, tray_pixels};

    /// The tray icon, its menu items and the unread mark it shows.
    pub struct Desktop {
        tray: Option<TrayIcon>,
        show: MenuId,
        quit: MenuId,
        unread: Option<bool>,
    }

    impl Desktop {
        /// Put the icon into the tray; without it closing the window quits.
        pub fn new() -> Desktop {
            let show = MenuItem::new("Открыть maxly-gpui", true, None);
            let quit = MenuItem::new("Выйти", true, None);
            let menu = Menu::new();
            let built = menu
                .append_items(&[&show, &PredefinedMenuItem::separator(), &quit])
                .ok()
                .and_then(|()| {
                    TrayIconBuilder::new()
                        .with_tooltip(tooltip(0))
                        .with_icon(icon(false)?)
                        .with_menu(Box::new(menu))
                        .with_menu_on_left_click(false)
                        .build()
                        .ok()
                });
            Desktop {
                tray: built,
                show: show.id().clone(),
                quit: quit.id().clone(),
                unread: None,
            }
        }

        /// The tray icon is there.
        pub fn has_tray(&self) -> bool {
            self.tray.is_some()
        }

        /// Clicks on the icon and its menu since the last call.
        pub fn poll(&self) -> Vec<DesktopEvent> {
            let mut out = Vec::new();
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    out.push(DesktopEvent::Show);
                }
            }
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id == self.show {
                    out.push(DesktopEvent::Show);
                } else if event.id == self.quit {
                    out.push(DesktopEvent::Quit);
                }
            }
            out
        }

        /// Show the unread count in the tooltip and a red dot on the icon.
        pub fn set_unread(&mut self, count: i32) {
            let Some(tray) = &self.tray else { return };
            let _ = tray.set_tooltip(Some(tooltip(count)));
            let mark = count > 0;
            if self.unread != Some(mark) {
                self.unread = Some(mark);
                let _ = tray.set_icon(icon(mark));
            }
        }
    }

    fn icon(unread: bool) -> Option<Icon> {
        Icon::from_rgba(tray_pixels(unread), ICON_SIZE, ICON_SIZE).ok()
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod tray {
    use super::DesktopEvent;

    /// No tray on this platform.
    pub struct Desktop;

    impl Desktop {
        pub fn new() -> Desktop {
            Desktop
        }
        pub fn has_tray(&self) -> bool {
            false
        }
        pub fn poll(&self) -> Vec<DesktopEvent> {
            Vec::new()
        }
        pub fn set_unread(&mut self, _count: i32) {}
    }
}

pub use tray::Desktop;

#[cfg(windows)]
mod win {
    use winsafe::{self as w, co};

    /// GPUI's window class on Windows.
    const GPUI_WINDOW_CLASS: &str = "Zed::Window";

    /// Run `f` on every top-level GPUI window of this process.
    fn each_window(mut f: impl FnMut(&w::HWND)) {
        let pid = w::GetCurrentProcessId();
        let _ = w::EnumWindows(|hwnd: w::HWND| {
            if hwnd.GetWindowThreadProcessId().1 == pid
                && hwnd.GetClassName().is_ok_and(|c| c == GPUI_WINDOW_CLASS)
            {
                f(&hwnd);
            }
            true
        });
    }

    /// Hide the window (it stays alive in the tray).
    pub fn conceal() {
        each_window(|hwnd| {
            hwnd.ShowWindow(co::SW::HIDE);
        });
    }

    /// Show, restore and raise the window.
    pub fn reveal() {
        each_window(|hwnd| {
            hwnd.ShowWindow(if hwnd.IsIconic() {
                co::SW::RESTORE
            } else {
                co::SW::SHOW
            });
            hwnd.SetForegroundWindow();
        });
    }

    /// The window is on screen (not hidden to the tray).
    pub fn visible() -> bool {
        let mut visible = false;
        each_window(|hwnd| visible |= hwnd.IsWindowVisible());
        visible
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Set when the close button minimized the window, cleared when it is shown again.
    /// Wayland does not always report a minimized window as hidden.
    static CONCEALED: AtomicBool = AtomicBool::new(false);

    pub fn conceal(window: &mut gpui_kit::Window) {
        CONCEALED.store(true, Ordering::Relaxed);
        window.minimize_window();
    }

    pub fn reveal(window: &mut gpui_kit::Window) {
        CONCEALED.store(false, Ordering::Relaxed);
        window.activate_window();
    }

    pub fn note_shown() {
        CONCEALED.store(false, Ordering::Relaxed);
    }

    pub fn visible() -> bool {
        !CONCEALED.load(Ordering::Relaxed)
    }
}

/// Hide the window, or minimize it where the platform cannot unmap it.
pub fn conceal(window: &mut gpui_kit::Window) {
    #[cfg(windows)]
    {
        let _ = window;
        win::conceal();
    }
    #[cfg(target_os = "linux")]
    linux::conceal(window);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = window;
    }
}

/// Show and raise the window.
pub fn reveal(window: &mut gpui_kit::Window) {
    #[cfg(windows)]
    {
        let _ = window;
        win::reveal();
    }
    #[cfg(target_os = "linux")]
    linux::reveal(window);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = window;
    }
}

/// The user brought the window back themselves (a click on the taskbar).
pub fn note_shown() {
    #[cfg(target_os = "linux")]
    linux::note_shown();
}

/// The window is on screen, not concealed to the tray.
pub fn window_visible() -> bool {
    #[cfg(windows)]
    {
        win::visible()
    }
    #[cfg(target_os = "linux")]
    {
        linux::visible()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{ICON_SIZE, tooltip, tray_pixels};

    fn pixel(p: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * ICON_SIZE + x) * 4) as usize;
        [p[i], p[i + 1], p[i + 2], p[i + 3]]
    }

    #[test]
    fn icon_has_a_disc_a_bubble_and_a_dot_only_when_unread() {
        let plain = tray_pixels(false);
        assert_eq!(plain.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert_eq!(pixel(&plain, 0, 0)[3], 0, "corners are transparent");
        assert_eq!(
            pixel(&plain, 16, 14),
            [0xFF; 4],
            "white bubble in the middle"
        );
        assert_eq!(
            pixel(&plain, 16, 29),
            [0x2F, 0x7C, 0xF6, 0xFF],
            "blue disc below it"
        );
        assert_eq!(pixel(&plain, 26, 6), [0x2F, 0x7C, 0xF6, 0xFF]);
        let marked = tray_pixels(true);
        assert_eq!(
            pixel(&marked, 26, 6),
            [0xE5, 0x39, 0x35, 0xFF],
            "red dot top right"
        );
        assert_eq!(pixel(&marked, 16, 14), pixel(&plain, 16, 14));
    }

    #[test]
    fn tooltips() {
        assert_eq!(tooltip(0), "maxly-gpui");
        assert_eq!(tooltip(7), "maxly-gpui — непрочитанных: 7");
    }
}
