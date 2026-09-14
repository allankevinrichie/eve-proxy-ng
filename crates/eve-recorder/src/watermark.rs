//! Per-frame timestamp watermark drawn directly into the BGRA capture
//! buffer, bottom-right corner: a small translucent bar with white
//! 5x7 pixel-font text (`T+123.4s`). This burns the real capture time of
//! each frame into the pixels themselves, so the video remains
//! self-aligning with the jsonl streams even if encoding lags or frames
//! are dropped.

/// 5x7 pixel font for the watermark alphabet.
const FONT: &[(char, [u8; 7])] = &[
    ('0', [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110]),
    ('1', [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
    ('2', [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111]),
    ('3', [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110]),
    ('4', [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
    ('5', [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110]),
    ('6', [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110]),
    ('7', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000]),
    ('8', [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
    ('9', [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100]),
    ('.', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100]),
    ('+', [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000]),
    ('s', [0b00000, 0b00000, 0b01111, 0b10000, 0b01111, 0b00001, 0b11110]),
    ('T', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100]),
    (' ', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000]),
];

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;

/// Format the watermark text for a session-relative timestamp.
pub fn format_stamp(t_ms: u64) -> String {
    format!("T+{}.{:01}s", t_ms / 1000, (t_ms % 1000) / 100)
}

/// Draw `text` at the bottom-right corner of a BGRA buffer (stride =
/// width*4) with a translucent black backing bar. Pixel scale grows the
/// glyph so the stamp stays readable after video compression.
pub fn draw_stamp_bgra(pixels: &mut [u8], width: usize, height: usize, text: &str, scale: usize) {
    let char_w = (GLYPH_W + 1) * scale;
    let text_w = text.chars().count() * char_w;
    let text_h = GLYPH_H * scale;
    let pad = 2 * scale;
    let bar_w = text_w + 2 * pad;
    let bar_h = text_h + 2 * pad;
    if width < bar_w || height < bar_h {
        return;
    }
    let x0 = width - bar_w;
    let y0 = height - bar_h;

    // Translucent black backing (50% toward black, keep alpha).
    for y in 0..bar_h {
        for x in 0..bar_w {
            let i = ((y0 + y) * width + x0 + x) * 4;
            pixels[i] /= 2;
            pixels[i + 1] /= 2;
            pixels[i + 2] /= 2;
        }
    }

    // White glyphs.
    for (ci, ch) in text.chars().enumerate() {
        let glyph = FONT
            .iter()
            .find(|(c, _)| *c == ch)
            .map(|(_, g)| *g)
            .unwrap_or([0; 7]);
        let cx = x0 + pad + ci * char_w;
        for gy in 0..GLYPH_H {
            for gx in 0..GLYPH_W {
                if glyph[gy] & (1 << (GLYPH_W - 1 - gx)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = cx + gx * scale + sx;
                        let py = y0 + pad + gy * scale + sy;
                        let i = (py * width + px) * 4;
                        pixels[i] = 0xF0;
                        pixels[i + 1] = 0xF0;
                        pixels[i + 2] = 0xF0;
                        pixels[i + 3] = 0xFF;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_white(p: &[u8]) -> bool {
        p[0] > 0xE0 && p[1] > 0xE0 && p[2] > 0xE0
    }

    #[test]
    fn stamp_draws_only_bottom_right() {
        let (w, h) = (160usize, 64usize);
        let mut buf = vec![0x40u8; w * h * 4];
        draw_stamp_bgra(&mut buf, w, h, &format_stamp(12345), 2);
        // Stamp text is present somewhere in the bottom-right quadrant.
        let mut whites_in_br = 0;
        for y in (h / 2)..h {
            for x in (w / 2)..w {
                if is_white(&buf[(y * w + x) * 4..(y * w + x) * 4 + 4]) {
                    whites_in_br += 1;
                }
            }
        }
        assert!(whites_in_br > 10, "expected glyph pixels, got {whites_in_br}");
        // Top-left quadrant untouched.
        for y in 0..(h / 2) {
            for x in 0..(w / 2) {
                let i = (y * w + x) * 4;
                assert_eq!(buf[i], 0x40, "top-left modified at {x},{y}");
            }
        }
    }

    #[test]
    fn stamp_format_matches_pattern() {
        assert_eq!(format_stamp(123_456), "T+123.4s");
        assert_eq!(format_stamp(9), "T+0.0s");
    }

    #[test]
    fn tiny_buffers_are_skipped() {
        let mut buf = vec![0u8; 8 * 8 * 4];
        draw_stamp_bgra(&mut buf, 8, 8, "T+1.2s", 2);
        assert!(buf.iter().all(|&b| b == 0));
    }
}

/// NV12 stamp, split by plane so the caller can borrow Y and UV
/// separately: luma first (translucent backing + glyphs), then neutral
/// chroma under the bar.
pub fn draw_stamp_nv12_y(
    y_plane: &mut [u8],
    y_stride: usize,
    width: usize,
    height: usize,
    text: &str,
    scale: usize,
) -> (usize, usize, usize, usize) {
    let char_w = (GLYPH_W + 1) * scale;
    let text_w = text.chars().count() * char_w;
    let text_h = GLYPH_H * scale;
    let pad = 2 * scale;
    let bar_w = text_w + 2 * pad;
    let bar_h = text_h + 2 * pad;
    if width < bar_w || height < bar_h {
        return (0, 0, 0, 0);
    }
    let x0 = width - bar_w;
    let y0 = height - bar_h;
    for y in 0..bar_h {
        for x in 0..bar_w {
            let i = (y0 + y) * y_stride + x0 + x;
            if i < y_plane.len() {
                y_plane[i] = y_plane[i] / 2 + 16;
            }
        }
    }
    for (ci, ch) in text.chars().enumerate() {
        let glyph = FONT
            .iter()
            .find(|(c, _)| *c == ch)
            .map(|(_, g)| *g)
            .unwrap_or([0; 7]);
        let cx = x0 + pad + ci * char_w;
        for gy in 0..GLYPH_H {
            for gx in 0..GLYPH_W {
                if glyph[gy] & (1 << (GLYPH_W - 1 - gx)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let i = (y0 + pad + gy * scale + sy) * y_stride + cx + gx * scale + sx;
                        if i < y_plane.len() {
                            y_plane[i] = 235;
                        }
                    }
                }
            }
        }
    }
    (x0, y0, bar_w, bar_h)
}

/// Neutral (128) chroma for the stamp bar area (2x2 subsampled).
pub fn draw_stamp_nv12_uv(
    uv_plane: &mut [u8],
    uv_stride: usize,
    bar: (usize, usize, usize, usize),
) {
    let (x0, y0, bar_w, bar_h) = bar;
    if bar_w == 0 {
        return;
    }
    for uy in 0..(bar_h + 1) / 2 {
        for ux in 0..(bar_w + 1) / 2 {
            let col = x0 / 2 + ux;
            let row = y0 / 2 + uy;
            let i = row * uv_stride + col * 2;
            if i + 1 < uv_plane.len() {
                uv_plane[i] = 128;
                uv_plane[i + 1] = 128;
            }
        }
    }
}
