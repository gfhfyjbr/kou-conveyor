//! A surface's texture: a very quiet, fixed film grain laid over a large flat
//! fill, so the rail and the empty screens read as a material rather than as a
//! plane of one colour.
//!
//! The grain is one 128px tile of noise, drawn by the SVG renderer once per
//! display scale and kept in GPUI's sprite atlas; every frame after that only
//! places copies of it. It never moves and never changes, and it is painted in
//! the theme's `fg` at the strength the theme allows (see
//! [`crate::GRAIN_DARK`]), which the contrast tests check text against.

use crate::Theme;
use gpui::{
    Bounds, Canvas, ContentMask, SharedString, Styled as _, TransformationMatrix, canvas, point,
    px, size,
};

/// The side of one grain tile, in logical pixels. Tiles meet without a seam.
pub const GRAIN_TILE: f32 = 128.;

const GRAIN_SVG: &[u8] = include_bytes!("../surface/grain.svg");
// The atlas keys a rendered SVG by this name and its size; it is not a file
// the app's asset source has to know.
const GRAIN_KEY: &str = "kou-ui/surface/grain.svg";

/// A fixed film grain over the whole of its parent, beneath the parent's
/// other children.
///
/// Give the parent `.relative()` and make the grain its first child, so
/// everything after it paints on top:
///
/// ```ignore
/// div()
///     .relative()
///     .bg(theme.rail)
///     .child(kou_ui::grain(theme))
///     .child(rows)
/// ```
///
/// It is absolutely placed, so it takes no room in the parent's layout, and it
/// has no hitbox, so the pointer goes through it. It clips to its parent's
/// bounds. Lay it over large flat fills only (a rail, a screen's background),
/// not over each row: nested grains add up.
pub fn grain(theme: &Theme) -> Canvas<()> {
    let ink = theme.grain_ink();
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            if ink.a <= 0. {
                return;
            }
            let key = SharedString::new_static(GRAIN_KEY);
            let tile = px(GRAIN_TILE);
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let mut y = bounds.origin.y;
                while y < bounds.bottom() {
                    let mut x = bounds.origin.x;
                    while x < bounds.right() {
                        // A tile that cannot be drawn leaves the surface flat, which is
                        // what it was before; nothing to report to the user.
                        let _ = window.paint_svg(
                            Bounds::new(point(x, y), size(tile, tile)),
                            key.clone(),
                            Some(GRAIN_SVG),
                            TransformationMatrix::unit(),
                            ink,
                            cx,
                        );
                        x += tile;
                    }
                    y += tile;
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tile_is_a_seamless_square_of_noise_the_size_it_is_drawn_at() {
        let text = std::str::from_utf8(GRAIN_SVG).unwrap();
        let side = format!("viewBox=\"0 0 {GRAIN_TILE} {GRAIN_TILE}\"");
        assert!(text.starts_with("<svg ") && text.contains(&side), "{text}");
        // Stitched, so copies side by side meet without a line between them.
        assert!(text.contains("stitchTiles=\"stitch\""));
        // A fixed seed: the same grain on every run and every machine.
        assert!(text.contains("seed=\""));
    }
}
