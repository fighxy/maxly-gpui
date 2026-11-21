//! One frame of a Lottie file, drawn by ThorVG (the official client's `lottie_player` uses it).
//!
//! The only `unsafe` is handing ThorVG a pixel buffer it writes during `draw`/`sync`. The buffer
//! stays in this function until the canvas is finished and dropped, then the pixels are copied out.

use std::cell::RefCell;

use thorvg::{ColorSpace, EngineOption, MimeType, Paint as _, Thorvg};

thread_local! {
    static ENGINE: RefCell<Option<Thorvg>> = const { RefCell::new(None) };
}

/// A small filled circle, used by the demo and the tests. 30 frames, 64×64.
pub const PULSE: &str = r#"{"v":"5.7.4","fr":30,"ip":0,"op":30,"w":64,"h":64,"nm":"pulse","ddd":0,"assets":[],"layers":[{"ddd":0,"ind":1,"ty":4,"nm":"dot","sr":1,"ks":{"o":{"a":0,"k":100},"r":{"a":0,"k":0},"p":{"a":0,"k":[32,32,0]},"a":{"a":0,"k":[0,0,0]},"s":{"a":1,"k":[{"t":0,"s":[70,70,100]},{"t":15,"s":[110,110,100]},{"t":29,"s":[70,70,100]}]}},"ao":0,"shapes":[{"ty":"el","d":1,"s":{"a":0,"k":[28,28]},"p":{"a":0,"k":[0,0]},"nm":"e"},{"ty":"fl","c":{"a":0,"k":[0.12,0.45,0.95,1]},"o":{"a":0,"k":100},"nm":"f"}],"ip":0,"op":30,"st":0,"bm":0}]}"#;

/// How many frames `json` has. `None` when ThorVG cannot read it.
pub fn frame_count(json: &[u8]) -> Option<u32> {
    with_engine(|engine| {
        let mut anim = engine.animation().ok()?;
        anim.picture_mut()
            .load_data(json, MimeType::Lottie, None)
            .ok()?;
        let total = anim.total_frame().ok()?;
        Some(total.max(1.0).floor() as u32)
    })
}

/// Frame `index` of `json` as tightly packed BGRA, `size` pixels square. `None` when it cannot
/// be drawn. The bytes are straight (not premultiplied) BGRA, which is what the window atlas
/// stores.
pub fn render_frame(json: &[u8], index: u32, size: u32) -> Option<Vec<u8>> {
    let mut frames = render_frames(json, size, Some(index))?;
    frames.pop()
}

/// Every frame of `json`, loaded once. `only` draws that single frame instead.
pub fn render_frames(json: &[u8], size: u32, only: Option<u32>) -> Option<Vec<Vec<u8>>> {
    if size == 0 || json.is_empty() {
        return None;
    }
    with_engine(|engine| {
        let pixels = (size as usize).checked_mul(size as usize)?;
        let mut buffer = vec![0u32; pixels];
        let mut canvas = engine.sw_canvas(EngineOption::Default).ok()?;
        // The canvas writes `buffer` during draw and sync. It is dropped before the pixels are read.
        let drawn = unsafe {
            canvas
                .set_target(&mut buffer, size, size, size, ColorSpace::ABGR8888S)
                .is_ok()
        };
        if !drawn {
            return None;
        }
        let mut anim = engine.animation().ok()?;
        anim.picture_mut()
            .load_data(json, MimeType::Lottie, None)
            .ok()?;
        anim.picture_mut().set_size(size as f32, size as f32).ok()?;
        let total = anim.total_frame().unwrap_or(1.0).max(1.0);
        let count = total.floor().max(1.0) as u32;
        let start = only.unwrap_or(0).min(count - 1);
        let end = only.map_or(count, |_| start + 1);
        let mut frames = Vec::with_capacity((end - start) as usize);
        for index in start..end {
            canvas.clear().ok()?;
            let _ = anim.set_frame((index as f32).min(total - 1.0));
            let copy = anim.picture().duplicate()?;
            canvas.add(copy).ok()?;
            // `true` clears the pixel buffer before this frame is drawn.
            canvas.draw(true).ok()?;
            canvas.sync().ok()?;
            frames.push(straight_bgra(&buffer));
        }
        drop(canvas);
        Some(frames)
    })
}

/// ABGR8888S keeps R in the low byte. The window atlas is BGRA, so red and blue trade places.
fn straight_bgra(buffer: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buffer.len() * 4);
    for &pixel in buffer {
        let r = (pixel & 0xFF) as u8;
        let g = ((pixel >> 8) & 0xFF) as u8;
        let b = ((pixel >> 16) & 0xFF) as u8;
        let a = ((pixel >> 24) & 0xFF) as u8;
        out.extend_from_slice(&[b, g, r, a]);
    }
    out
}

fn with_engine<T>(body: impl FnOnce(&Thorvg) -> Option<T>) -> Option<T> {
    ENGINE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Thorvg::init(0).ok();
        }
        slot.as_ref().and_then(body)
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_pulse_draws_a_frame() {
        let json = PULSE.as_bytes();
        let count = frame_count(json).expect("frames");
        assert!(count >= 2, "the pulse has more than one frame");
        let frame = render_frame(json, 0, 32).expect("pixels");
        assert_eq!(frame.len(), 32 * 32 * 4);
        assert!(
            frame.chunks(4).any(|px| px[3] > 0),
            "a frame has a visible pixel"
        );
    }

    #[test]
    fn a_blue_dot_is_stored_with_blue_first() {
        // The pulse fill is blue. The atlas reads the first byte as blue.
        let frame = render_frame(PULSE.as_bytes(), 0, 32).expect("pixels");
        let pixel = frame
            .chunks(4)
            .filter(|px| px[3] > 200)
            .max_by_key(|px| u16::from(px[0]) + u16::from(px[1]) + u16::from(px[2]))
            .expect("an opaque pixel");
        assert!(
            pixel[0] > pixel[2],
            "blue is the first byte, red the third, got {pixel:?}"
        );
    }
}
