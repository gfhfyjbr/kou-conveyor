//! Box-drawing and block characters (U+2500 to U+259F) as geometry.
//!
//! A font's own glyphs for these stop at its ascent and descent, so at any line height above
//! one they leave a gap between rows and a table's borders turn into dashes. Drawing them
//! from the cell's own rectangle, in whole device pixels, makes neighbouring cells meet
//! exactly whatever the font and line height are.

/// One piece of a character, in device pixels from the cell's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BoxOp {
    /// A filled rectangle covering `[x0, x1)` by `[y0, y1)`.
    Rect {
        /// Left edge.
        x0: i32,
        /// Top edge.
        y0: i32,
        /// Right edge.
        x1: i32,
        /// Bottom edge.
        y1: i32,
        /// Coverage: 1 for solid shapes, less for the shade characters.
        alpha: f32,
    },
    /// A rounded corner joining the cell's horizontal and vertical centre lines.
    Arc {
        /// Whether the horizontal arm leaves through the right edge (else the left).
        right: bool,
        /// Whether the vertical arm leaves through the bottom edge (else the top).
        down: bool,
        /// Stroke thickness.
        thickness: i32,
    },
    /// A stroke between opposite corners.
    Diagonal {
        /// From bottom-left to top-right when true, from top-left to bottom-right otherwise.
        rising: bool,
        /// Stroke thickness.
        thickness: i32,
    },
}

/// Whether `character` is drawn from geometry instead of from the font.
pub(crate) fn is_drawn(character: char) -> bool {
    ('\u{2500}'..='\u{259f}').contains(&character)
}

const NONE: u8 = 0;
const LIGHT: u8 = 1;
const HEAVY: u8 = 2;
const DOUBLE: u8 = 3;

/// The weight of each arm, `[left, up, right, down]`, for U+2500 to U+257F. Dashed lines,
/// arcs and diagonals have no arms here; [`ops`] handles them before consulting the table.
#[rustfmt::skip]
const ARMS: [[u8; 4]; 128] = [
    // U+2500
    [1,0,1,0], [2,0,2,0], [0,1,0,1], [0,2,0,2], [0,0,0,0], [0,0,0,0], [0,0,0,0], [0,0,0,0],
    [0,0,0,0], [0,0,0,0], [0,0,0,0], [0,0,0,0], [0,0,1,1], [0,0,2,1], [0,0,1,2], [0,0,2,2],
    // U+2510
    [1,0,0,1], [2,0,0,1], [1,0,0,2], [2,0,0,2], [0,1,1,0], [0,1,2,0], [0,2,1,0], [0,2,2,0],
    [1,1,0,0], [2,1,0,0], [1,2,0,0], [2,2,0,0], [0,1,1,1], [0,1,2,1], [0,2,1,1], [0,1,1,2],
    // U+2520
    [0,2,1,2], [0,2,2,1], [0,1,2,2], [0,2,2,2], [1,1,0,1], [2,1,0,1], [1,2,0,1], [1,1,0,2],
    [1,2,0,2], [2,2,0,1], [2,1,0,2], [2,2,0,2], [1,0,1,1], [2,0,1,1], [1,0,2,1], [2,0,2,1],
    // U+2530
    [1,0,1,2], [2,0,1,2], [1,0,2,2], [2,0,2,2], [1,1,1,0], [2,1,1,0], [1,1,2,0], [2,1,2,0],
    [1,2,1,0], [2,2,1,0], [1,2,2,0], [2,2,2,0], [1,1,1,1], [2,1,1,1], [1,1,2,1], [2,1,2,1],
    // U+2540
    [1,2,1,1], [1,1,1,2], [1,2,1,2], [2,2,1,1], [1,2,2,1], [2,1,1,2], [1,1,2,2], [2,2,2,1],
    [2,1,2,2], [2,2,1,2], [1,2,2,2], [2,2,2,2], [0,0,0,0], [0,0,0,0], [0,0,0,0], [0,0,0,0],
    // U+2550
    [3,0,3,0], [0,3,0,3], [0,0,3,1], [0,0,1,3], [0,0,3,3], [3,0,0,1], [1,0,0,3], [3,0,0,3],
    [0,1,3,0], [0,3,1,0], [0,3,3,0], [3,1,0,0], [1,3,0,0], [3,3,0,0], [0,1,3,1], [0,3,1,3],
    // U+2560
    [0,3,3,3], [3,1,0,1], [1,3,0,3], [3,3,0,3], [3,0,3,1], [1,0,1,3], [3,0,3,3], [3,1,3,0],
    [1,3,1,0], [3,3,3,0], [3,1,3,1], [1,3,1,3], [3,3,3,3], [0,0,0,0], [0,0,0,0], [0,0,0,0],
    // U+2570
    [0,0,0,0], [0,0,0,0], [0,0,0,0], [0,0,0,0], [1,0,0,0], [0,1,0,0], [0,0,1,0], [0,0,0,1],
    [2,0,0,0], [0,2,0,0], [0,0,2,0], [0,0,0,2], [1,0,2,0], [0,1,0,2], [2,0,1,0], [0,2,0,1],
];

/// The thickness of a light line for a cell `width` device pixels wide.
pub(crate) fn light_thickness(width: i32) -> i32 {
    ((width + 4) / 8).max(1)
}

/// Appends the pieces of `character` for a cell of `width` by `height` device pixels.
/// Does nothing for characters outside the range [`is_drawn`] accepts.
pub(crate) fn ops(character: char, width: i32, height: i32, out: &mut Vec<BoxOp>) {
    let code = character as u32;
    let light = light_thickness(width);
    match code {
        0x2500..=0x257f => lines(code - 0x2500, width, height, light, out),
        0x2580..=0x259f => blocks(code - 0x2580, width, height, out),
        _ => {}
    }
}

fn rect(out: &mut Vec<BoxOp>, x0: i32, y0: i32, x1: i32, y1: i32) {
    if x1 > x0 && y1 > y0 {
        out.push(BoxOp::Rect {
            x0,
            y0,
            x1,
            y1,
            alpha: 1.,
        });
    }
}

fn lines(index: u32, width: i32, height: i32, light: i32, out: &mut Vec<BoxOp>) {
    let heavy = light * 2;
    match index {
        0x04 => return dashes(true, 3, light, width, height, out),
        0x05 => return dashes(true, 3, heavy, width, height, out),
        0x06 => return dashes(false, 3, light, width, height, out),
        0x07 => return dashes(false, 3, heavy, width, height, out),
        0x08 => return dashes(true, 4, light, width, height, out),
        0x09 => return dashes(true, 4, heavy, width, height, out),
        0x0a => return dashes(false, 4, light, width, height, out),
        0x0b => return dashes(false, 4, heavy, width, height, out),
        0x4c => return dashes(true, 2, light, width, height, out),
        0x4d => return dashes(true, 2, heavy, width, height, out),
        0x4e => return dashes(false, 2, light, width, height, out),
        0x4f => return dashes(false, 2, heavy, width, height, out),
        0x6d..=0x70 => {
            let (right, down) = match index {
                0x6d => (true, true),
                0x6e => (false, true),
                0x6f => (false, false),
                _ => (true, false),
            };
            out.push(BoxOp::Arc {
                right,
                down,
                thickness: light,
            });
            return;
        }
        0x71..=0x73 => {
            if index != 0x72 {
                out.push(BoxOp::Diagonal {
                    rising: true,
                    thickness: light,
                });
            }
            if index != 0x71 {
                out.push(BoxOp::Diagonal {
                    rising: false,
                    thickness: light,
                });
            }
            return;
        }
        _ => {}
    }
    let arms = ARMS[index as usize];
    if arms.contains(&DOUBLE) {
        double_lines(arms, width, height, light, out);
    } else {
        single_lines(arms, width, height, light, out);
    }
}

/// Light and heavy lines: each arm runs from its edge to the far side of the stroke it
/// meets, so corners are square and solid whatever the two weights are.
fn single_lines(arms: [u8; 4], width: i32, height: i32, light: i32, out: &mut Vec<BoxOp>) {
    let [left, up, right, down] = arms;
    let thickness = |weight: u8| match weight {
        NONE => 0,
        HEAVY => light * 2,
        _ => light,
    };
    // The span a centred stroke of a given thickness occupies along an axis.
    let across = |extent: i32, thickness: i32| {
        let start = (extent - thickness) / 2;
        (start, start + thickness)
    };
    let vertical = thickness(up.max(down));
    let horizontal = thickness(left.max(right));
    let middle_x = across(width, light);
    let middle_y = across(height, light);

    if left != NONE {
        let (y0, y1) = across(height, thickness(left));
        let end = if vertical > 0 {
            across(width, vertical).1
        } else if right != NONE {
            width / 2
        } else {
            middle_x.1
        };
        rect(out, 0, y0, end, y1);
    }
    if right != NONE {
        let (y0, y1) = across(height, thickness(right));
        let start = if vertical > 0 {
            across(width, vertical).0
        } else if left != NONE {
            width / 2
        } else {
            middle_x.0
        };
        rect(out, start, y0, width, y1);
    }
    if up != NONE {
        let (x0, x1) = across(width, thickness(up));
        let end = if horizontal > 0 {
            across(height, horizontal).1
        } else if down != NONE {
            height / 2
        } else {
            middle_y.1
        };
        rect(out, x0, 0, x1, end);
    }
    if down != NONE {
        let (x0, x1) = across(width, thickness(down));
        let start = if horizontal > 0 {
            across(height, horizontal).0
        } else if up != NONE {
            height / 2
        } else {
            middle_y.0
        };
        rect(out, x0, start, x1, height);
    }
}

/// Double lines, alone or meeting light ones.
///
/// The middle of the cell is a 3x3 grid of squares one light stroke wide. A double arm
/// owns the outer two tracks of that grid and a light arm the middle one; how far each
/// stroke runs into the grid depends on what it meets, which is what makes inner and outer
/// corners, tees and crossings come out right from one rule.
fn double_lines(arms: [u8; 4], width: i32, height: i32, light: i32, out: &mut Vec<BoxOp>) {
    let x0 = (width - 3 * light) / 2;
    let y0 = (height - 3 * light) / 2;
    let xs = [x0, x0 + light, x0 + 2 * light, x0 + 3 * light];
    let ys = [y0, y0 + light, y0 + 2 * light, y0 + 3 * light];
    let mut filled = [[false; 3]; 3];

    for direction in 0..4 {
        let weight = arms[direction];
        if weight == NONE {
            continue;
        }
        let horizontal = direction % 2 == 0;
        let opposite = arms[(direction + 2) % 4];
        // The perpendicular arms on the low (left or top) and high side of this arm.
        let (low, high) = if horizontal {
            (arms[1], arms[3])
        } else {
            (arms[0], arms[2])
        };
        // Tracks this arm draws on, with the perpendicular arm each one faces.
        let tracks: &[(usize, u8, u8)] = if weight == DOUBLE {
            &[(0, low, high), (2, high, low)]
        } else {
            &[(1, NONE, NONE)]
        };
        for &(track, near, far) in tracks {
            // Outside the grid the stroke always runs from the cell's edge.
            let (from, to) = match direction {
                0 => (0, xs[0]),
                1 => (0, ys[0]),
                2 => (xs[3], width),
                _ => (ys[3], height),
            };
            if horizontal {
                rect(out, from, ys[track], to, ys[track + 1]);
            } else {
                rect(out, xs[track], from, xs[track + 1], to);
            }

            let reach = if weight == DOUBLE {
                match (near, opposite, far) {
                    // An inner corner: stop at the double arm this stroke faces.
                    (DOUBLE, _, _) => 1,
                    (LIGHT, _, _) => 2,
                    // Nothing on this side: run through, or round the outside of a corner.
                    (_, opposite, _) if opposite != NONE => 3,
                    (_, _, DOUBLE) => 3,
                    _ => 2,
                }
            } else if opposite != NONE {
                3
            } else {
                match u8::from(low != NONE) + u8::from(high != NONE) {
                    // A tee: the crossing line passes, this one only touches it.
                    2 => 1,
                    // A corner: reach the far stroke of the arm it turns into.
                    1 => 3,
                    _ => 2,
                }
            };
            for step in 0..reach {
                // Arms entering from the right or the bottom walk the grid backwards.
                let along = if direction >= 2 { 2 - step } else { step };
                if horizontal {
                    filled[track][along] = true;
                } else {
                    filled[along][track] = true;
                }
            }
        }
    }

    for (row, cells) in filled.iter().enumerate() {
        for (column, filled) in cells.iter().enumerate() {
            if *filled {
                rect(out, xs[column], ys[row], xs[column + 1], ys[row + 1]);
            }
        }
    }
}

fn dashes(
    horizontal: bool,
    count: i32,
    thickness: i32,
    width: i32,
    height: i32,
    out: &mut Vec<BoxOp>,
) {
    let length = if horizontal { width } else { height };
    let gap = (length / (count * 3)).max(1);
    for index in 0..count {
        let start = index * length / count + gap / 2;
        let end = (index + 1) * length / count - (gap - gap / 2);
        if horizontal {
            let y0 = (height - thickness) / 2;
            rect(out, start, y0, end, y0 + thickness);
        } else {
            let x0 = (width - thickness) / 2;
            rect(out, x0, start, x0 + thickness, end);
        }
    }
}

fn blocks(index: u32, width: i32, height: i32, out: &mut Vec<BoxOp>) {
    // Eighths rounded to the nearest pixel, never to nothing.
    let eighths = |extent: i32, count: i32| ((extent * count + 4) / 8).max(1);
    let half_x = width / 2;
    let half_y = height / 2;
    let quadrants = |out: &mut Vec<BoxOp>, upper_left, upper_right, lower_left, lower_right| {
        if upper_left {
            rect(out, 0, 0, half_x, half_y);
        }
        if upper_right {
            rect(out, half_x, 0, width, half_y);
        }
        if lower_left {
            rect(out, 0, half_y, half_x, height);
        }
        if lower_right {
            rect(out, half_x, half_y, width, height);
        }
    };
    match index {
        0x00 => rect(out, 0, 0, width, half_y),
        0x01..=0x08 => rect(
            out,
            0,
            height - eighths(height, index as i32),
            width,
            height,
        ),
        0x09..=0x0f => rect(out, 0, 0, eighths(width, 0x10 - index as i32), height),
        0x10 => rect(out, half_x, 0, width, height),
        0x11..=0x13 => out.push(BoxOp::Rect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
            alpha: 0.25 * (index - 0x10) as f32,
        }),
        0x14 => rect(out, 0, 0, width, eighths(height, 1)),
        0x15 => rect(out, width - eighths(width, 1), 0, width, height),
        0x16 => quadrants(out, false, false, true, false),
        0x17 => quadrants(out, false, false, false, true),
        0x18 => quadrants(out, true, false, false, false),
        0x19 => quadrants(out, true, false, true, true),
        0x1a => quadrants(out, true, false, false, true),
        0x1b => quadrants(out, true, true, true, false),
        0x1c => quadrants(out, true, true, false, true),
        0x1d => quadrants(out, false, true, false, false),
        0x1e => quadrants(out, false, true, true, false),
        0x1f => quadrants(out, false, true, true, true),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: i32 = 16;
    const HEIGHT: i32 = 36;

    /// Rasterises a character's rectangles into a coverage grid.
    fn raster(character: char) -> Vec<Vec<bool>> {
        let mut out = Vec::new();
        ops(character, WIDTH, HEIGHT, &mut out);
        let mut grid = vec![vec![false; WIDTH as usize]; HEIGHT as usize];
        for op in out {
            if let BoxOp::Rect { x0, y0, x1, y1, .. } = op {
                assert!(x0 >= 0 && y0 >= 0 && x1 <= WIDTH && y1 <= HEIGHT, "{op:?}");
                for row in &mut grid[y0 as usize..y1 as usize] {
                    for cell in &mut row[x0 as usize..x1 as usize] {
                        *cell = true;
                    }
                }
            }
        }
        grid
    }

    fn column(grid: &[Vec<bool>], x: usize) -> Vec<bool> {
        grid.iter().map(|row| row[x]).collect()
    }

    #[test]
    fn vertical_lines_span_the_cell_and_line_up() {
        let bar = raster('│');
        // The stroke touches both edges, so stacked cells join without a gap.
        assert!(bar[0].iter().any(|filled| *filled));
        assert!(bar[HEIGHT as usize - 1].iter().any(|filled| *filled));
        assert_eq!(bar[0], bar[HEIGHT as usize - 1]);
        // A corner and a tee leave through the bottom edge on the same pixels.
        for character in ['┌', '┐', '├', '┤', '┬', '┼', '╷'] {
            assert_eq!(
                raster(character)[HEIGHT as usize - 1],
                bar[0],
                "{character}"
            );
        }
        for character in ['└', '┘', '├', '┤', '┴', '┼', '╵'] {
            assert_eq!(raster(character)[0], bar[0], "{character}");
        }
    }

    #[test]
    fn horizontal_lines_span_the_cell_and_line_up() {
        let dash = raster('─');
        let left = column(&dash, 0);
        assert!(left.iter().any(|filled| *filled));
        assert_eq!(left, column(&dash, WIDTH as usize - 1));
        for character in ['┌', '└', '├', '┬', '┴', '┼', '╶'] {
            assert_eq!(
                column(&raster(character), WIDTH as usize - 1),
                left,
                "{character}"
            );
        }
        for character in ['┐', '┘', '┤', '┬', '┴', '┼', '╴'] {
            assert_eq!(column(&raster(character), 0), left, "{character}");
        }
    }

    #[test]
    fn double_lines_line_up_with_each_other() {
        let rail = raster('║');
        assert_eq!(rail[0].iter().filter(|filled| **filled).count(), 4);
        for character in ['╔', '╗', '╠', '╣', '╦', '╬'] {
            assert_eq!(
                raster(character)[HEIGHT as usize - 1],
                rail[0],
                "{character}"
            );
        }
        let beam = column(&raster('═'), 0);
        for character in ['╔', '╚', '╠', '╦', '╩', '╬'] {
            assert_eq!(
                column(&raster(character), WIDTH as usize - 1),
                beam,
                "{character}"
            );
        }
    }

    #[test]
    fn double_corner_has_an_outer_and_an_inner_stroke() {
        let corner = raster('╔');
        let light = light_thickness(WIDTH) as usize;
        let x0 = (WIDTH as usize - 3 * light) / 2;
        let y0 = (HEIGHT as usize - 3 * light) / 2;
        // The outer corner square is filled, the middle of the grid is not.
        assert!(corner[y0][x0]);
        assert!(!corner[y0 + light][x0 + light]);
        // The inner corner is where the two inner strokes meet.
        assert!(corner[y0 + 2 * light][x0 + 2 * light]);
        // Nothing is drawn towards the edges the character does not reach.
        assert!(!corner[0].iter().any(|filled| *filled));
        assert!(!column(&corner, 0).iter().any(|filled| *filled));
    }

    #[test]
    fn double_crossing_keeps_its_centre_open() {
        let crossing = raster('╬');
        assert!(!crossing[HEIGHT as usize / 2][WIDTH as usize / 2]);
    }

    #[test]
    fn mixed_tee_touches_the_line_it_meets() {
        // A light arm into a double vertical: the arm reaches the nearer rail.
        let tee = raster('╟');
        let row = &tee[HEIGHT as usize / 2];
        let first_gap_after_rail = row.iter().rposition(|filled| !*filled);
        // Everything right of the right rail is filled along the arm's row.
        assert!(first_gap_after_rail.is_some_and(|gap| gap < WIDTH as usize / 2 + 1));
    }

    #[test]
    fn blocks_cover_the_expected_part() {
        let full = raster('█');
        assert!(full.iter().all(|row| row.iter().all(|filled| *filled)));
        let lower_half = raster('▄');
        assert!(!lower_half[0][0]);
        assert!(lower_half[HEIGHT as usize - 1][0]);
        assert_eq!(
            lower_half.iter().filter(|row| row[0]).count(),
            HEIGHT as usize / 2
        );
        let left_half = raster('▌');
        assert_eq!(
            left_half[0].iter().filter(|filled| **filled).count(),
            WIDTH as usize / 2
        );
        let quadrant = raster('▚');
        assert!(quadrant[0][0] && quadrant[HEIGHT as usize - 1][WIDTH as usize - 1]);
        assert!(!quadrant[0][WIDTH as usize - 1] && !quadrant[HEIGHT as usize - 1][0]);
    }

    #[test]
    fn shades_are_translucent() {
        let mut out = Vec::new();
        ops('▒', WIDTH, HEIGHT, &mut out);
        assert_eq!(
            out,
            vec![BoxOp::Rect {
                x0: 0,
                y0: 0,
                x1: WIDTH,
                y1: HEIGHT,
                alpha: 0.5
            }]
        );
    }

    #[test]
    fn every_character_in_range_draws_something() {
        for code in 0x2500..=0x259f_u32 {
            let character = char::from_u32(code).unwrap();
            let mut out = Vec::new();
            ops(character, WIDTH, HEIGHT, &mut out);
            assert!(!out.is_empty(), "U+{code:04X} draws nothing");
            // A one-pixel-per-eighth cell must not panic or go out of bounds either.
            let mut small = Vec::new();
            ops(character, 5, 9, &mut small);
            for op in small {
                if let BoxOp::Rect { x0, y0, x1, y1, .. } = op {
                    assert!(
                        x0 >= 0 && y0 >= 0 && x1 <= 5 && y1 <= 9,
                        "U+{code:04X} {op:?}"
                    );
                }
            }
        }
    }
}
