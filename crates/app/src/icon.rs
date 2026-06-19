//! Programmatic app icon: two cascading "screens" on a blue rounded tile,
//! rendered to RGBA at any size with signed-distance-field antialiasing. Used for
//! both the tray icon and the window icon, so no binary asset is required.
//!
//! The same design lives as a vector source in `assets/icon.svg` (used to make
//! the `.ico` for the executable/installer).

/// Render the icon to non-premultiplied RGBA8 (`size * size * 4` bytes).
pub fn rgba(size: u32) -> Vec<u8> {
    let n = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];

    // Geometry in pixel units, scaled to `size`.
    let tile_r = n * 0.22;
    let tile_half = n * 0.5 - n * 0.06;
    let (cx, cy) = (n * 0.5, n * 0.5);

    let screen_half = n * 0.255;
    let screen_r = n * 0.06;
    let back = (n * 0.40, n * 0.40);
    let front = (n * 0.60, n * 0.60);
    let gap = n * 0.05; // separation halo cut between the two screens

    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            // Start transparent, composite back-to-front.
            let mut acc = [0.0f32; 4];

            // 1) Blue rounded tile with a vertical gradient.
            let d_tile = sd_round_rect(px, py, cx, cy, tile_half, tile_half, tile_r);
            let g = (py / n).clamp(0.0, 1.0);
            let tile = [
                lerp(0.180, 0.106, g), // R: #2E… → #1B…
                lerp(0.471, 0.302, g), // G
                lerp(0.863, 0.659, g), // B
                coverage(d_tile),
            ];
            over(&mut acc, tile);

            // 2) Back screen (white).
            let d_back = sd_round_rect(px, py, back.0, back.1, screen_half, screen_half, screen_r);
            over(&mut acc, [1.0, 1.0, 1.0, coverage(d_back)]);

            // 3) Separation halo: front screen grown by `gap`, painted in the tile
            //    color so the two screens read as distinct.
            let d_halo = sd_round_rect(
                px,
                py,
                front.0,
                front.1,
                screen_half + gap,
                screen_half + gap,
                screen_r + gap,
            );
            over(&mut acc, [tile[0], tile[1], tile[2], coverage(d_halo)]);

            // 4) Front screen (white).
            let d_front =
                sd_round_rect(px, py, front.0, front.1, screen_half, screen_half, screen_r);
            over(&mut acc, [1.0, 1.0, 1.0, coverage(d_front)]);

            let i = ((y * size + x) * 4) as usize;
            out[i] = to_u8(acc[0]);
            out[i + 1] = to_u8(acc[1]);
            out[i + 2] = to_u8(acc[2]);
            out[i + 3] = to_u8(acc[3]);
        }
    }
    out
}

/// Signed distance to a rounded rectangle (negative inside).
fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r
}

/// Convert a signed distance to 0..1 coverage with ~1px antialiasing.
fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Straight-alpha source-over compositing of `src` onto `dst`.
fn over(dst: &mut [f32; 4], src: [f32; 4]) {
    let sa = src[3];
    if sa <= 0.0 {
        return;
    }
    let prev_a = dst[3];
    let out_a = sa + prev_a * (1.0 - sa);
    if out_a <= 0.0 {
        *dst = [0.0; 4];
        return;
    }
    for (d, s) in dst.iter_mut().zip(src.iter()).take(3) {
        *d = (s * sa + *d * prev_a * (1.0 - sa)) / out_a;
    }
    dst[3] = out_a;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_expected_size_and_some_opaque_pixels() {
        let px = rgba(64);
        assert_eq!(px.len(), 64 * 64 * 4);
        // Center pixel should be opaque (on the tile).
        let center = ((32 * 64 + 32) * 4 + 3) as usize;
        assert!(px[center] > 250);
        // A corner should be transparent (outside the rounded tile).
        assert_eq!(px[3], 0);
    }
}
