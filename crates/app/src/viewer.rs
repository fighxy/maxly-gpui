//! The full-screen photo viewer's state and geometry: which photo is shown, its rotation, zoom
//! and drag offset, and the pixel rotation itself. The drawing lives in `app_view`.

/// One photo the viewer can show.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewerPhoto {
    /// Image address (the key of the loaded image).
    pub url: String,
    /// Server photo id, for the saved file name.
    pub photo_id: Option<i64>,
    /// When the message was sent, Unix ms.
    pub time: i64,
    /// Who sent it.
    pub sender: String,
}

/// The open viewer.
#[derive(Debug, Clone)]
pub struct Viewer {
    /// Every photo of the chat as far as it is loaded, oldest first.
    pub photos: Vec<ViewerPhoto>,
    /// The one shown.
    pub index: usize,
    /// Clockwise quarter turns, 0..4.
    pub turns: u8,
    /// Scale over the fitted size; 1 shows the whole photo.
    pub zoom: f32,
    /// Drag offset from the centre, px.
    pub pan: (f32, f32),
    /// The pointer position of a drag in progress.
    pub drag: Option<(f32, f32)>,
    /// A line shown in the toolbar, such as where a download was saved.
    pub notice: Option<String>,
}

/// Each zoom step multiplies or divides by this.
pub const ZOOM_STEP: f32 = 1.25;
/// Smallest zoom over the fitted size.
pub const ZOOM_MIN: f32 = 0.25;
/// Largest zoom over the fitted size.
pub const ZOOM_MAX: f32 = 10.;

impl Viewer {
    /// Open on `index` of `photos`, fitted, upright.
    pub fn new(photos: Vec<ViewerPhoto>, index: usize) -> Viewer {
        let index = index.min(photos.len().saturating_sub(1));
        Viewer {
            photos,
            index,
            turns: 0,
            zoom: 1.,
            pan: (0., 0.),
            drag: None,
            notice: None,
        }
    }

    /// The photo shown.
    pub fn current(&self) -> Option<&ViewerPhoto> {
        self.photos.get(self.index)
    }

    /// Go `by` photos forward (or back), staying in range; a new photo starts fitted and upright.
    pub fn step(&mut self, by: isize) {
        let last = self.photos.len().saturating_sub(1) as isize;
        let next = (self.index as isize + by).clamp(0, last) as usize;
        if next != self.index {
            self.index = next;
            self.notice = None;
            self.reset();
        }
    }

    /// Back to fitted, upright, centred.
    pub fn reset(&mut self) {
        self.turns = 0;
        self.zoom = 1.;
        self.pan = (0., 0.);
        self.drag = None;
    }

    /// Turn a quarter clockwise; the turned photo is fitted again.
    pub fn rotate(&mut self) {
        self.turns = (self.turns + 1) % 4;
        self.zoom = 1.;
        self.pan = (0., 0.);
    }

    /// Zoom in (`steps` > 0) or out, keeping the offset in proportion so the same spot stays
    /// near the centre.
    pub fn zoom_by(&mut self, steps: i32) {
        let before = self.zoom;
        self.zoom = (self.zoom * ZOOM_STEP.powi(steps)).clamp(ZOOM_MIN, ZOOM_MAX);
        let k = self.zoom / before;
        self.pan = (self.pan.0 * k, self.pan.1 * k);
    }
}

/// The scale that fits `image` (w, h) into `area`, never above 1: small photos show at their
/// own size.
pub fn fit_scale(image: (f32, f32), area: (f32, f32)) -> f32 {
    if image.0 <= 0. || image.1 <= 0. {
        return 1.;
    }
    (area.0 / image.0).min(area.1 / image.1).min(1.)
}

/// Keep a drag offset where the shown photo still covers the middle: no offset along a side
/// where the photo fits, at most the overhang where it is larger.
pub fn clamp_pan(pan: (f32, f32), shown: (f32, f32), area: (f32, f32)) -> (f32, f32) {
    let limit = |shown: f32, area: f32| ((shown - area) / 2.).max(0.);
    let (lx, ly) = (limit(shown.0, area.0), limit(shown.1, area.1));
    (pan.0.clamp(-lx, lx), pan.1.clamp(-ly, ly))
}

/// Turn a 4-byte-per-pixel image `turns` quarter turns clockwise; returns the pixels and the new
/// width and height.
pub fn rotate_pixels(pixels: &[u8], width: u32, height: u32, turns: u8) -> (Vec<u8>, u32, u32) {
    let (w, h) = (width as usize, height as usize);
    if pixels.len() < w * h * 4 {
        return (pixels.to_vec(), width, height);
    }
    let turns = turns % 4;
    if turns == 0 {
        return (pixels.to_vec(), width, height);
    }
    let (nw, nh) = if turns % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match turns {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            let src = (y * w + x) * 4;
            let dst = (ny * nw + nx) * 4;
            out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
        }
    }
    (out, nw as u32, nh as u32)
}

#[cfg(test)]
mod tests {
    use super::{Viewer, ViewerPhoto, clamp_pan, fit_scale, rotate_pixels};

    fn photos(n: usize) -> Vec<ViewerPhoto> {
        (0..n)
            .map(|i| ViewerPhoto {
                url: format!("u{i}"),
                photo_id: None,
                time: 0,
                sender: String::new(),
            })
            .collect()
    }

    /// 2x1 image: pixel A then pixel B, one byte value each.
    fn px(v: u8) -> [u8; 4] {
        [v, v, v, 255]
    }

    #[test]
    fn rotation_moves_pixels_clockwise() {
        // A B
        // C D
        let img: Vec<u8> = [px(1), px(2), px(3), px(4)].concat();
        let (r, w, h) = rotate_pixels(&img, 2, 2, 1);
        // C A
        // D B
        assert_eq!((w, h), (2, 2));
        assert_eq!(r, [px(3), px(1), px(4), px(2)].concat());
        let (r, _, _) = rotate_pixels(&img, 2, 2, 2);
        assert_eq!(r, [px(4), px(3), px(2), px(1)].concat());
        let (r, _, _) = rotate_pixels(&img, 2, 2, 3);
        assert_eq!(r, [px(2), px(4), px(1), px(3)].concat());
        // A wide image turns tall.
        let wide: Vec<u8> = [px(1), px(2), px(3)].concat();
        let (r, w, h) = rotate_pixels(&wide, 3, 1, 1);
        assert_eq!((w, h), (1, 3));
        assert_eq!(r, wide, "a single row read top to bottom");
        assert_eq!(rotate_pixels(&wide, 3, 1, 4).0, wide, "four turns are none");
    }

    #[test]
    fn fitting_and_drag_limits() {
        assert_eq!(fit_scale((2000., 1000.), (1000., 800.)), 0.5);
        assert_eq!(
            fit_scale((400., 300.), (1000., 800.)),
            1.,
            "small photos keep their size"
        );
        assert_eq!(clamp_pan((50., 50.), (800., 600.), (1000., 800.)), (0., 0.));
        assert_eq!(
            clamp_pan((500., -500.), (1600., 1200.), (1000., 800.)),
            (300., -200.)
        );
    }

    #[test]
    fn stepping_zoom_and_rotation() {
        let mut v = Viewer::new(photos(3), 5);
        assert_eq!(v.index, 2, "clamped into range");
        v.zoom_by(2);
        v.pan = (10., 0.);
        v.step(-1);
        assert_eq!(
            (v.index, v.zoom, v.pan),
            (1, 1., (0., 0.)),
            "a new photo starts fitted"
        );
        v.step(-5);
        assert_eq!(v.index, 0);
        v.zoom_by(1);
        v.pan = (8., 4.);
        v.zoom_by(1);
        assert!((v.pan.0 - 10.).abs() < 1e-4, "offset scales with the zoom");
        v.zoom_by(-100);
        assert_eq!(v.zoom, super::ZOOM_MIN);
        for _ in 0..4 {
            v.rotate();
        }
        assert_eq!(v.turns, 0);
        assert!(Viewer::new(Vec::new(), 0).current().is_none());
    }
}
