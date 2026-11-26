//! Box-drawing (U+2500..U+257F) and block elements (U+2580..U+259F) drawn as
//! rectangles that fill the whole cell.
//!
//! Font glyphs for these characters span the font's own line height, so at
//! the design's 1.6 line height the lines of a `tmux` border or a table break
//! between rows. Other terminals (Alacritty, WezTerm, Windows Terminal) draw
//! them the same way. Dashed lines and diagonals are left to the font.

/// Segment weights per box-drawing character, two bits per direction:
/// up (bits 0-1), down (2-3), left (4-5), right (6-7). 0 none, 1 light,
/// 2 heavy, 3 double. A zero entry is left to the font.
/// Generated from the Unicode names; rounded corners are drawn square.
const BOX: [u8; 128] = [
    0x50, 0xA0, 0x05, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x44, 0x84, 0x48, 0x88, //
    0x14, 0x24, 0x18, 0x28, 0x41, 0x81, 0x42, 0x82, 0x11, 0x21, 0x12, 0x22, 0x45, 0x85, 0x46, 0x49, //
    0x4A, 0x86, 0x89, 0x8A, 0x15, 0x25, 0x16, 0x19, 0x1A, 0x26, 0x29, 0x2A, 0x54, 0x64, 0x94, 0xA4, //
    0x58, 0x68, 0x98, 0xA8, 0x51, 0x61, 0x91, 0xA1, 0x52, 0x62, 0x92, 0xA2, 0x55, 0x65, 0x95, 0xA5, //
    0x56, 0x59, 0x5A, 0x66, 0x96, 0x69, 0x99, 0xA6, 0xA9, 0x6A, 0x9A, 0xAA, 0x00, 0x00, 0x00, 0x00, //
    0xF0, 0x0F, 0xC4, 0x4C, 0xCC, 0x34, 0x1C, 0x3C, 0xC1, 0x43, 0xC3, 0x31, 0x13, 0x33, 0xC5, 0x4F, //
    0xCF, 0x35, 0x1F, 0x3F, 0xF4, 0x5C, 0xFC, 0xF1, 0x53, 0xF3, 0xF5, 0x5F, 0xFF, 0x44, 0x14, 0x11, //
    0x41, 0x00, 0x00, 0x00, 0x10, 0x01, 0x40, 0x04, 0x20, 0x02, 0x80, 0x08, 0x90, 0x09, 0x60, 0x06, //
];

/// A rectangle in cell units (0..1 across, 0..1 down) with an opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub alpha: f32,
}

/// Whether `c` is drawn here rather than by the font.
pub fn handles(c: char) -> bool {
    match c as u32 {
        cp @ 0x2500..=0x257F => BOX[(cp - 0x2500) as usize] != 0,
        0x2580..=0x259F => true,
        _ => false,
    }
}

/// Rectangles for `c` in a cell `cell_w` by `cell_h` pixels, in pixels from
/// the cell's top-left corner. `light` is the light line thickness.
pub fn rects(c: char, cell_w: f32, cell_h: f32, light: f32) -> Vec<Rect> {
    let cp = c as u32;
    if (0x2580..=0x259F).contains(&cp) {
        return block(cp, cell_w, cell_h);
    }
    if !(0x2500..=0x257F).contains(&cp) {
        return Vec::new();
    }
    let bits = BOX[(cp - 0x2500) as usize];
    let [up, down, left, right] = [bits & 3, (bits >> 2) & 3, (bits >> 4) & 3, (bits >> 6) & 3];
    let heavy = (light * 2.0).round().max(light + 1.0);
    let thick = |w: u8| if w == 2 { heavy } else { light };
    // Double lines: two light lines this far either side of the centre.
    let gap = light.max(1.0);
    let (cx, cy) = ((cell_w / 2.0).floor(), (cell_h / 2.0).floor());
    let mut out = Vec::new();
    let mut push = |x: f32, y: f32, w: f32, h: f32| out.push(Rect { x, y, w, h, alpha: 1.0 });

    // Half-width of the vertical and horizontal strokes at the centre, so
    // horizontal segments reach across the vertical stroke and joins close.
    let v_half = match up.max(down) {
        0 => 0.0,
        3 => gap + light / 2.0,
        w => thick(w) / 2.0,
    };
    let h_half = match left.max(right) {
        0 => 0.0,
        3 => gap + light / 2.0,
        w => thick(w) / 2.0,
    };

    for (w, upward) in [(up, true), (down, false)] {
        if w == 0 {
            continue;
        }
        let (y0, y1) = if upward { (0.0, cy + h_half) } else { (cy - h_half, cell_h) };
        let xs: Vec<f32> =
            if w == 3 { vec![cx - gap - light / 2.0, cx + gap - light / 2.0] } else { vec![cx - thick(w) / 2.0] };
        for x in xs {
            push(x, y0, if w == 3 { light } else { thick(w) }, y1 - y0);
        }
    }
    for (w, leftward) in [(left, true), (right, false)] {
        if w == 0 {
            continue;
        }
        let (x0, x1) = if leftward { (0.0, cx + v_half) } else { (cx - v_half, cell_w) };
        let ys: Vec<f32> =
            if w == 3 { vec![cy - gap - light / 2.0, cy + gap - light / 2.0] } else { vec![cy - thick(w) / 2.0] };
        for y in ys {
            push(x0, y, x1 - x0, if w == 3 { light } else { thick(w) });
        }
    }
    out
}

fn block(cp: u32, w: f32, h: f32) -> Vec<Rect> {
    let r = |x: f32, y: f32, rw: f32, rh: f32| Rect { x: x * w, y: y * h, w: rw * w, h: rh * h, alpha: 1.0 };
    let shade = |alpha: f32| vec![Rect { x: 0.0, y: 0.0, w, h, alpha }];
    let (ul, ur, ll, lr) = (r(0.0, 0.0, 0.5, 0.5), r(0.5, 0.0, 0.5, 0.5), r(0.0, 0.5, 0.5, 0.5), r(0.5, 0.5, 0.5, 0.5));
    match cp {
        0x2580 => vec![r(0.0, 0.0, 1.0, 0.5)],
        // Lower one to seven eighths, then the full block.
        0x2581..=0x2588 => {
            let f = (cp - 0x2580) as f32 / 8.0;
            vec![r(0.0, 1.0 - f, 1.0, f)]
        }
        // Left seven eighths down to one eighth.
        0x2589..=0x258F => {
            let f = (0x2590 - cp) as f32 / 8.0;
            vec![r(0.0, 0.0, f, 1.0)]
        }
        0x2590 => vec![r(0.5, 0.0, 0.5, 1.0)],
        0x2591 => shade(0.25),
        0x2592 => shade(0.5),
        0x2593 => shade(0.75),
        0x2594 => vec![r(0.0, 0.0, 1.0, 0.125)],
        0x2595 => vec![r(0.875, 0.0, 0.125, 1.0)],
        0x2596 => vec![ll],
        0x2597 => vec![lr],
        0x2598 => vec![ul],
        0x2599 => vec![ul, ll, lr],
        0x259A => vec![ul, lr],
        0x259B => vec![ul, ur, ll],
        0x259C => vec![ul, ur, lr],
        0x259D => vec![ur],
        0x259E => vec![ur, ll],
        0x259F => vec![ur, ll, lr],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rectangles' union touches these cell edges.
    fn edges(c: char) -> (bool, bool, bool, bool) {
        let (w, h) = (10.0, 20.0);
        let rs = rects(c, w, h, 1.0);
        (
            rs.iter().any(|r| r.y <= 0.0),
            rs.iter().any(|r| r.y + r.h >= h),
            rs.iter().any(|r| r.x <= 0.0),
            rs.iter().any(|r| r.x + r.w >= w),
        )
    }

    #[test]
    fn lines_reach_the_edges_they_name() {
        assert_eq!(edges('─'), (false, false, true, true));
        assert_eq!(edges('│'), (true, true, false, false));
        assert_eq!(edges('┌'), (false, true, false, true));
        assert_eq!(edges('┘'), (true, false, true, false));
        assert_eq!(edges('┼'), (true, true, true, true));
        assert_eq!(edges('╠'), (true, true, false, true));
        assert_eq!(edges('╰'), (true, false, false, true));
        assert_eq!(edges('╴'), (false, false, true, false));
    }

    #[test]
    fn weights_and_doubles() {
        // Heavy is thicker than light; double is two strokes.
        let light = rects('─', 10.0, 20.0, 1.0);
        let heavy = rects('━', 10.0, 20.0, 1.0);
        assert!(heavy[0].h > light[0].h);
        // Two strokes for each half.
        assert_eq!(rects('═', 10.0, 20.0, 1.0).len(), 4);
        assert_eq!(rects('║', 10.0, 20.0, 1.0).len(), 4);
        // ┍: light down, heavy right.
        let r = rects('┍', 10.0, 20.0, 1.0);
        let down = r.iter().find(|r| r.y + r.h >= 20.0).unwrap();
        let right = r.iter().find(|r| r.x + r.w >= 10.0).unwrap();
        assert!(right.h > down.w);
    }

    #[test]
    fn dashes_and_diagonals_stay_with_the_font() {
        for c in ['┄', '┅', '╌', '╱', '╲', '╳', 'a', '█'] {
            assert_eq!(handles(c), c == '█', "{c}");
        }
    }

    #[test]
    fn blocks() {
        assert_eq!(rects('█', 8.0, 16.0, 1.0), vec![Rect { x: 0.0, y: 0.0, w: 8.0, h: 16.0, alpha: 1.0 }]);
        assert_eq!(rects('▄', 8.0, 16.0, 1.0)[0], Rect { x: 0.0, y: 8.0, w: 8.0, h: 8.0, alpha: 1.0 });
        assert_eq!(rects('▌', 8.0, 16.0, 1.0)[0], Rect { x: 0.0, y: 0.0, w: 4.0, h: 16.0, alpha: 1.0 });
        assert_eq!(rects('▒', 8.0, 16.0, 1.0)[0].alpha, 0.5);
        assert_eq!(rects('▙', 8.0, 16.0, 1.0).len(), 3);
    }
}
