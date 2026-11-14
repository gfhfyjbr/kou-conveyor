//! Mouse reports for programs that track the pointer (X10, button and any-motion tracking)
//! in the SGR (1006) and the original byte encodings.

use gpui::Modifiers;

/// What the program asked to hear about the mouse, and in which encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MouseMode {
    /// Presses and releases (modes 9 and 1000, and the two below).
    pub click: bool,
    /// Motion while a button is held (mode 1002, and 1003).
    pub drag: bool,
    /// All motion (mode 1003).
    pub motion: bool,
    /// The SGR encoding (mode 1006).
    pub sgr: bool,
    /// The UTF-8 extension of the original encoding (mode 1005).
    pub utf8: bool,
}

impl MouseMode {
    /// Whether the program tracks the mouse at all.
    pub(crate) fn tracking(&self) -> bool {
        self.click || self.drag || self.motion
    }
}

/// What the pointer did, in the numbering the protocol uses for its button field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseAction {
    /// A button went down: 0 left, 1 middle, 2 right.
    Press(u8),
    /// A button went up.
    Release(u8),
    /// The pointer moved to another cell, with the held button if there is one.
    Motion(Option<u8>),
    /// One wheel step: up or down.
    Wheel {
        /// Whether the step scrolls towards the top.
        up: bool,
    },
}

/// The report for `action` at a zero-based cell, or `None` when the program's mode does not
/// ask for this kind of event or the encoding cannot express the position.
pub(crate) fn report(
    action: MouseAction,
    column: usize,
    line: usize,
    modifiers: Modifiers,
    mode: MouseMode,
) -> Option<Vec<u8>> {
    if !mode.tracking() {
        return None;
    }
    let (button, motion, release) = match action {
        MouseAction::Press(button) => (button, false, false),
        MouseAction::Release(button) => (button, false, true),
        MouseAction::Motion(button) => {
            let wanted = match button {
                Some(_) => mode.drag || mode.motion,
                None => mode.motion,
            };
            if !wanted {
                return None;
            }
            // Motion with no button held is reported as button 3.
            (button.unwrap_or(3), true, false)
        }
        MouseAction::Wheel { up } => (if up { 64 } else { 65 }, false, false),
    };
    let mut code = u32::from(button);
    if motion {
        code += 32;
    }
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }

    if mode.sgr {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", column + 1, line + 1).into_bytes());
    }

    // The original encoding has no way to say which button was released.
    if release {
        code = (code & !3) | 3;
    }
    let mut bytes = vec![0x1b, b'[', b'M'];
    let utf8 = mode.utf8;
    for value in [code, column as u32 + 1, line as u32 + 1] {
        let value = value + 32;
        if value < 128 {
            bytes.push(value as u8);
        } else if utf8 && value < 0x800 {
            bytes.push(0xc0 | (value >> 6) as u8);
            bytes.push(0x80 | (value & 0x3f) as u8);
        } else if !utf8 && value < 256 {
            bytes.push(value as u8);
        } else {
            return None;
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLICK: MouseMode = MouseMode {
        click: true,
        drag: false,
        motion: false,
        sgr: false,
        utf8: false,
    };
    const SGR: MouseMode = MouseMode { sgr: true, ..CLICK };

    const NONE: Modifiers = Modifiers {
        control: false,
        alt: false,
        shift: false,
        platform: false,
        function: false,
    };

    fn text(bytes: Option<Vec<u8>>) -> Option<String> {
        bytes.map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    #[test]
    fn nothing_without_a_mouse_mode() {
        assert_eq!(
            report(
                MouseAction::Press(0),
                0,
                0,
                NONE,
                MouseMode {
                    sgr: true,
                    ..MouseMode::default()
                }
            ),
            None
        );
    }

    #[test]
    fn sgr_press_release_and_wheel() {
        let mode = SGR;
        assert_eq!(
            text(report(MouseAction::Press(0), 4, 2, NONE, mode)),
            Some("\x1b[<0;5;3M".into())
        );
        assert_eq!(
            text(report(MouseAction::Release(2), 4, 2, NONE, mode)),
            Some("\x1b[<2;5;3m".into())
        );
        assert_eq!(
            text(report(MouseAction::Wheel { up: true }, 0, 0, NONE, mode)),
            Some("\x1b[<64;1;1M".into())
        );
        assert_eq!(
            text(report(MouseAction::Wheel { up: false }, 0, 0, NONE, mode)),
            Some("\x1b[<65;1;1M".into())
        );
        let control = Modifiers {
            control: true,
            ..NONE
        };
        assert_eq!(
            text(report(MouseAction::Press(0), 0, 0, control, mode)),
            Some("\x1b[<16;1;1M".into())
        );
    }

    #[test]
    fn motion_needs_the_matching_mode() {
        let click = SGR;
        let drag = MouseMode { drag: true, ..SGR };
        let any = MouseMode {
            drag: true,
            motion: true,
            ..SGR
        };
        assert_eq!(
            report(MouseAction::Motion(Some(0)), 1, 1, NONE, click),
            None
        );
        assert_eq!(
            text(report(MouseAction::Motion(Some(0)), 1, 1, NONE, drag)),
            Some("\x1b[<32;2;2M".into())
        );
        assert_eq!(report(MouseAction::Motion(None), 1, 1, NONE, drag), None);
        assert_eq!(
            text(report(MouseAction::Motion(None), 1, 1, NONE, any)),
            Some("\x1b[<35;2;2M".into())
        );
    }

    #[test]
    fn original_encoding() {
        let mode = CLICK;
        assert_eq!(
            report(MouseAction::Press(0), 0, 0, NONE, mode),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            report(MouseAction::Release(0), 9, 4, NONE, mode),
            Some(vec![0x1b, b'[', b'M', 35, 42, 37])
        );
        // Column 300 does not fit in a byte.
        assert_eq!(report(MouseAction::Press(0), 299, 0, NONE, mode), None);
        assert_eq!(
            report(
                MouseAction::Press(0),
                299,
                0,
                NONE,
                MouseMode { utf8: true, ..mode }
            ),
            Some(vec![0x1b, b'[', b'M', 32, 0xc5, 0x8c, 33])
        );
    }
}
