//! A QR code drawn as a grid of cells (no image decoding, crisp at any scale).

use gpui_kit::*;
use qrcode::{Color, QrCode};

/// The QR modules of `text`, `true` = dark; `None` if the text does not fit a QR code.
pub fn modules(text: &str) -> Option<(usize, Vec<bool>)> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let cells = code
        .to_colors()
        .into_iter()
        .map(|c| c == Color::Dark)
        .collect();
    Some((width, cells))
}

/// The code with a 4-module quiet zone, each module `cell` pixels.
pub fn view(text: &str, cell: f32) -> AnyElement {
    let Some((width, cells)) = modules(text) else {
        return div().child("QR-код не помещается").into_any_element();
    };
    let quiet = 4.0;
    let side = (width as f32 + quiet * 2.0) * cell;
    let mut rows = Vec::with_capacity(width);
    for y in 0..width {
        let row = div().flex().flex_row().children((0..width).map(|x| {
            let dark = cells[y * width + x];
            div()
                .w(px(cell))
                .h(px(cell))
                .bg(if dark { rgb(0x000000) } else { rgb(0xffffff) })
        }));
        rows.push(row);
    }
    div()
        .w(px(side))
        .h(px(side))
        .bg(rgb(0xffffff))
        .p(px(quiet * cell))
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::modules;

    #[test]
    fn encodes_a_login_link() {
        let (width, cells) =
            modules("https://max.ru/:auth/0123456789abcdef0123456789abcdef").unwrap();
        assert!(width >= 21);
        assert_eq!(cells.len(), width * width);
        // Finder pattern: the top-left corner module is dark.
        assert!(cells[0]);
    }
}
