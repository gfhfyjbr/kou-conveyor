//! Keystroke to byte-sequence translation.
//!
//! Two encodings: the xterm-style legacy one every program understands, and the Kitty
//! keyboard protocol's "disambiguate escape codes" level, which a program switches on to
//! tell Shift+Enter from Enter or Alt+letter from Escape followed by a letter.
//!
//! Keys that only produce text return `None`: they reach the program through the platform's
//! text input path, so composition and non-Latin layouts work.

use gpui::{Keystroke, Modifiers};

/// The terminal state that changes what a key sends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct KeyMode {
    /// DECCKM: unmodified arrows, Home and End use the SS3 prefix.
    pub app_cursor: bool,
    /// The program enabled any level of the Kitty keyboard protocol.
    pub kitty: bool,
    /// The program asked for Enter, Tab and Backspace as escape codes too (protocol flag 8).
    pub kitty_all_keys: bool,
    /// Alt (Option on macOS) acts as Meta instead of typing the layout's alternate characters.
    pub alt_is_meta: bool,
}

/// The final byte of a CSI sequence for a functional key, and the number before it.
#[derive(Clone, Copy)]
enum Functional {
    /// `CSI 1 ; mods <letter>`, or `CSI <letter>` unmodified.
    Letter(u8),
    /// `CSI <number> ; mods ~`.
    Tilde(u8),
}

/// The bytes a keystroke sends to the program, or `None` when it is a text key (or a
/// platform shortcut) that this layer does not encode.
pub(crate) fn encode(keystroke: &Keystroke, mode: KeyMode) -> Option<Vec<u8>> {
    let modifiers = keystroke.modifiers;
    if modifiers.platform {
        // Command and Super chords belong to the application around the terminal.
        return None;
    }
    let key = keystroke.key.as_str();
    let modifier_code = modifier_code(modifiers);

    match key {
        "enter" => return Some(encode_c0(b'\r', 13, modifiers, mode)),
        "tab" => return Some(encode_c0(b'\t', 9, modifiers, mode)),
        "backspace" => return Some(encode_c0(0x7f, 127, modifiers, mode)),
        "escape" => {
            return Some(if mode.kitty {
                csi_u(27, modifier_code)
            } else if modifiers.alt {
                vec![0x1b, 0x1b]
            } else {
                vec![0x1b]
            });
        }
        "space" => return encode_text_key(' ', modifiers, mode),
        _ => {}
    }

    if let Some(functional) = functional_key(key, mode.kitty) {
        return Some(encode_functional(key, functional, modifiers, mode));
    }

    let mut characters = key.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => encode_text_key(character, modifiers, mode),
        _ => None,
    }
}

/// The xterm modifier parameter: one plus a bit per held modifier.
fn modifier_code(modifiers: Modifiers) -> u8 {
    1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.control)
}

fn csi_u(code: u32, modifier_code: u8) -> Vec<u8> {
    if modifier_code == 1 {
        format!("\x1b[{code}u").into_bytes()
    } else {
        format!("\x1b[{code};{modifier_code}u").into_bytes()
    }
}

/// Enter, Tab and Backspace: a control byte in the legacy encoding, and in the Kitty
/// protocol too unless a modifier is held, so a shell left in protocol mode stays usable.
fn encode_c0(byte: u8, code: u32, modifiers: Modifiers, mode: KeyMode) -> Vec<u8> {
    let modified = modifiers.shift || modifiers.alt || modifiers.control;
    if mode.kitty && (modified || mode.kitty_all_keys) {
        return csi_u(code, modifier_code(modifiers));
    }
    match (byte, modifiers.shift, modifiers.control) {
        // Back-tab has had its own sequence since before modifiers were encoded.
        (b'\t', true, _) => return b"\x1b[Z".to_vec(),
        // The newline Claude Code and others accept in place of a protocol-level
        // Shift+Enter; Alt+Enter below produces the same bytes.
        (b'\r', true, false) => return b"\x1b\r".to_vec(),
        // Ctrl+Enter is a line feed, as Ctrl+J would be: programs that take Ctrl+J for
        // "new line without sending" then treat both alike.
        (b'\r', _, true) => {
            return if modifiers.alt {
                b"\x1b\n".to_vec()
            } else {
                b"\n".to_vec()
            };
        }
        (0x7f, _, true) => {
            return if modifiers.alt {
                b"\x1b\x08".to_vec()
            } else {
                b"\x08".to_vec()
            };
        }
        _ => {}
    }
    if modifiers.alt {
        vec![0x1b, byte]
    } else {
        vec![byte]
    }
}

fn functional_key(key: &str, kitty: bool) -> Option<Functional> {
    Some(match key {
        "up" => Functional::Letter(b'A'),
        "down" => Functional::Letter(b'B'),
        "right" => Functional::Letter(b'C'),
        "left" => Functional::Letter(b'D'),
        "home" => Functional::Letter(b'H'),
        "end" => Functional::Letter(b'F'),
        "insert" => Functional::Tilde(2),
        "delete" => Functional::Tilde(3),
        "pageup" => Functional::Tilde(5),
        "pagedown" => Functional::Tilde(6),
        "f1" => Functional::Letter(b'P'),
        "f2" => Functional::Letter(b'Q'),
        // `CSI R` is also the cursor position report, so the Kitty protocol moved F3.
        "f3" if kitty => Functional::Tilde(13),
        "f3" => Functional::Letter(b'R'),
        "f4" => Functional::Letter(b'S'),
        "f5" => Functional::Tilde(15),
        "f6" => Functional::Tilde(17),
        "f7" => Functional::Tilde(18),
        "f8" => Functional::Tilde(19),
        "f9" => Functional::Tilde(20),
        "f10" => Functional::Tilde(21),
        "f11" => Functional::Tilde(23),
        "f12" => Functional::Tilde(24),
        "f13" => Functional::Tilde(25),
        "f14" => Functional::Tilde(26),
        "f15" => Functional::Tilde(28),
        "f16" => Functional::Tilde(29),
        "f17" => Functional::Tilde(31),
        "f18" => Functional::Tilde(32),
        "f19" => Functional::Tilde(33),
        "f20" => Functional::Tilde(34),
        _ => return None,
    })
}

fn encode_functional(
    key: &str,
    functional: Functional,
    modifiers: Modifiers,
    mode: KeyMode,
) -> Vec<u8> {
    let modifier_code = modifier_code(modifiers);
    if modifier_code == 1 {
        return match functional {
            Functional::Letter(letter) => {
                let function_key = matches!(letter, b'P'..=b'S');
                // F1 to F4 are SS3 in every mode; the cursor keys only while the program
                // asked for application cursor keys and is not on the Kitty protocol,
                // which ignores that mode.
                if function_key || (mode.app_cursor && !mode.kitty) {
                    vec![0x1b, b'O', letter]
                } else {
                    vec![0x1b, b'[', letter]
                }
            }
            Functional::Tilde(number) => format!("\x1b[{number}~").into_bytes(),
        };
    }
    if !mode.kitty && mode.alt_is_meta && modifiers.alt && !modifiers.control && !modifiers.shift {
        // Word motion the way readline and zsh bind it out of the box; the xterm form
        // `CSI 1;3D` is unbound in a default macOS shell and prints as garbage.
        match key {
            "left" => return b"\x1bb".to_vec(),
            "right" => return b"\x1bf".to_vec(),
            _ => {}
        }
    }
    match functional {
        Functional::Letter(letter) => {
            format!("\x1b[1;{modifier_code}{}", letter as char).into_bytes()
        }
        Functional::Tilde(number) => format!("\x1b[{number};{modifier_code}~").into_bytes(),
    }
}

/// A key that types a character: encoded only when Ctrl or Alt changes its meaning.
fn encode_text_key(character: char, modifiers: Modifiers, mode: KeyMode) -> Option<Vec<u8>> {
    let alt = modifiers.alt && mode.alt_is_meta;
    if !modifiers.control && !alt {
        return (character == ' ').then(|| vec![b' ']);
    }
    if !character.is_ascii() {
        // A layout without Latin letters reports them through `key` already; anything
        // else non-ASCII has no control or Meta form.
        return None;
    }

    if mode.kitty {
        let mut code = modifier_code(modifiers);
        if !alt && modifiers.alt {
            code -= 2;
        }
        // The protocol identifies a key by its unshifted character.
        return Some(csi_u(u32::from(character.to_ascii_lowercase()), code));
    }

    let mut bytes = Vec::with_capacity(2);
    if alt {
        bytes.push(0x1b);
    }
    if modifiers.control {
        bytes.push(control_byte(character)?);
    } else if modifiers.shift {
        bytes.push(character.to_ascii_uppercase() as u8);
    } else {
        bytes.push(character as u8);
    }
    Some(bytes)
}

/// The C0 control character Ctrl produces with a key, following xterm.
fn control_byte(character: char) -> Option<u8> {
    Some(match character.to_ascii_lowercase() {
        letter @ 'a'..='z' => letter as u8 - b'a' + 1,
        ' ' | '@' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' | '~' | '`' => 0x1e,
        '_' | '7' | '-' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// Whether the terminal takes this keystroke before the application's key bindings do.
///
/// Plain and Shift keys, Ctrl with a character and Alt with a key are what a terminal
/// program expects to own: a global binding on Tab or Escape must not take them from it.
/// Chords with more modifiers (Ctrl+Tab, Ctrl+Shift+letter) are left to the application,
/// and reach the terminal only when nothing is bound to them.
pub(crate) fn is_claimed(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    if modifiers.platform || modifiers.function {
        return false;
    }
    match (modifiers.control, modifiers.alt) {
        (false, false) => true,
        (true, false) => !modifiers.shift && keystroke.key.chars().count() == 1,
        (false, true) => !modifiers.shift,
        (true, true) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY: KeyMode = KeyMode {
        app_cursor: false,
        kitty: false,
        kitty_all_keys: false,
        alt_is_meta: true,
    };
    const APP_CURSOR: KeyMode = KeyMode {
        app_cursor: true,
        ..LEGACY
    };
    const KITTY: KeyMode = KeyMode {
        kitty: true,
        ..LEGACY
    };

    fn sent(source: &'static str, mode: KeyMode) -> Option<String> {
        let keystroke = Keystroke::parse(source).expect("a valid keystroke");
        encode(&keystroke, mode).map(|bytes| String::from_utf8(bytes).expect("ASCII"))
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    #[test]
    fn plain_keys() {
        assert_eq!(sent("enter", LEGACY), some("\r"));
        assert_eq!(sent("tab", LEGACY), some("\t"));
        assert_eq!(sent("backspace", LEGACY), some("\x7f"));
        assert_eq!(sent("escape", LEGACY), some("\x1b"));
        assert_eq!(sent("space", LEGACY), some(" "));
        assert_eq!(sent("up", LEGACY), some("\x1b[A"));
        assert_eq!(sent("down", LEGACY), some("\x1b[B"));
        assert_eq!(sent("right", LEGACY), some("\x1b[C"));
        assert_eq!(sent("left", LEGACY), some("\x1b[D"));
        assert_eq!(sent("home", LEGACY), some("\x1b[H"));
        assert_eq!(sent("end", LEGACY), some("\x1b[F"));
        assert_eq!(sent("insert", LEGACY), some("\x1b[2~"));
        assert_eq!(sent("delete", LEGACY), some("\x1b[3~"));
        assert_eq!(sent("pageup", LEGACY), some("\x1b[5~"));
        assert_eq!(sent("pagedown", LEGACY), some("\x1b[6~"));
        assert_eq!(sent("f1", LEGACY), some("\x1bOP"));
        assert_eq!(sent("f4", LEGACY), some("\x1bOS"));
        assert_eq!(sent("f5", LEGACY), some("\x1b[15~"));
        assert_eq!(sent("f12", LEGACY), some("\x1b[24~"));
    }

    #[test]
    fn text_keys_are_left_to_text_input() {
        assert_eq!(sent("a", LEGACY), None);
        assert_eq!(sent("shift-a", LEGACY), None);
        assert_eq!(sent("/", LEGACY), None);
        assert_eq!(sent("a", KITTY), None);
        // Command chords are the application's.
        assert_eq!(sent("cmd-c", LEGACY), None);
        assert_eq!(sent("cmd-enter", LEGACY), None);
    }

    #[test]
    fn control_characters() {
        assert_eq!(sent("ctrl-c", LEGACY), some("\x03"));
        assert_eq!(sent("ctrl-a", LEGACY), some("\x01"));
        assert_eq!(sent("ctrl-z", LEGACY), some("\x1a"));
        assert_eq!(sent("ctrl-shift-c", LEGACY), some("\x03"));
        assert_eq!(sent("ctrl-space", LEGACY), some("\0"));
        assert_eq!(sent("ctrl-[", LEGACY), some("\x1b"));
        assert_eq!(sent("ctrl-\\", LEGACY), some("\x1c"));
        assert_eq!(sent("ctrl-]", LEGACY), some("\x1d"));
        assert_eq!(sent("ctrl-/", LEGACY), some("\x1f"));
        assert_eq!(sent("ctrl-backspace", LEGACY), some("\x08"));
        assert_eq!(sent("ctrl-enter", LEGACY), some("\n"));
    }

    #[test]
    fn alt_is_an_escape_prefix() {
        assert_eq!(sent("alt-b", LEGACY), some("\x1bb"));
        assert_eq!(sent("alt-shift-b", LEGACY), some("\x1bB"));
        assert_eq!(sent("alt-.", LEGACY), some("\x1b."));
        assert_eq!(sent("alt-enter", LEGACY), some("\x1b\r"));
        assert_eq!(sent("alt-backspace", LEGACY), some("\x1b\x7f"));
        assert_eq!(sent("ctrl-alt-c", LEGACY), some("\x1b\x03"));
        assert_eq!(sent("alt-left", LEGACY), some("\x1bb"));
        assert_eq!(sent("alt-right", LEGACY), some("\x1bf"));
        assert_eq!(sent("alt-up", LEGACY), some("\x1b[1;3A"));
    }

    #[test]
    fn alt_types_characters_when_it_is_not_meta() {
        let typing = KeyMode {
            alt_is_meta: false,
            ..LEGACY
        };
        assert_eq!(sent("alt-b", typing), None);
        assert_eq!(sent("ctrl-c", typing), some("\x03"));
    }

    #[test]
    fn shift_enter_and_back_tab() {
        assert_eq!(sent("shift-enter", LEGACY), some("\x1b\r"));
        assert_eq!(sent("shift-tab", LEGACY), some("\x1b[Z"));
        assert_eq!(sent("shift-backspace", LEGACY), some("\x7f"));
    }

    #[test]
    fn modified_functional_keys() {
        assert_eq!(sent("shift-up", LEGACY), some("\x1b[1;2A"));
        assert_eq!(sent("ctrl-right", LEGACY), some("\x1b[1;5C"));
        assert_eq!(sent("ctrl-shift-left", LEGACY), some("\x1b[1;6D"));
        assert_eq!(sent("ctrl-alt-down", LEGACY), some("\x1b[1;7B"));
        assert_eq!(sent("shift-home", LEGACY), some("\x1b[1;2H"));
        assert_eq!(sent("ctrl-delete", LEGACY), some("\x1b[3;5~"));
        assert_eq!(sent("shift-f1", LEGACY), some("\x1b[1;2P"));
        assert_eq!(sent("ctrl-f5", LEGACY), some("\x1b[15;5~"));
        assert_eq!(sent("f3", LEGACY), some("\x1bOR"));
        assert_eq!(sent("shift-f3", LEGACY), some("\x1b[1;2R"));
    }

    #[test]
    fn application_cursor_mode() {
        assert_eq!(sent("up", APP_CURSOR), some("\x1bOA"));
        assert_eq!(sent("left", APP_CURSOR), some("\x1bOD"));
        assert_eq!(sent("home", APP_CURSOR), some("\x1bOH"));
        assert_eq!(sent("end", APP_CURSOR), some("\x1bOF"));
        // Modified keys and the editing pad keep the CSI form.
        assert_eq!(sent("shift-up", APP_CURSOR), some("\x1b[1;2A"));
        assert_eq!(sent("pageup", APP_CURSOR), some("\x1b[5~"));
    }

    #[test]
    fn kitty_disambiguates() {
        assert_eq!(sent("escape", KITTY), some("\x1b[27u"));
        assert_eq!(sent("shift-enter", KITTY), some("\x1b[13;2u"));
        assert_eq!(sent("ctrl-enter", KITTY), some("\x1b[13;5u"));
        assert_eq!(sent("alt-enter", KITTY), some("\x1b[13;3u"));
        assert_eq!(sent("shift-tab", KITTY), some("\x1b[9;2u"));
        assert_eq!(sent("ctrl-backspace", KITTY), some("\x1b[127;5u"));
        assert_eq!(sent("alt-b", KITTY), some("\x1b[98;3u"));
        assert_eq!(sent("ctrl-c", KITTY), some("\x1b[99;5u"));
        assert_eq!(sent("ctrl-shift-c", KITTY), some("\x1b[99;6u"));
        assert_eq!(sent("ctrl-alt-x", KITTY), some("\x1b[120;7u"));
        assert_eq!(sent("ctrl-space", KITTY), some("\x1b[32;5u"));
        assert_eq!(sent("alt-left", KITTY), some("\x1b[1;3D"));
        assert_eq!(sent("f3", KITTY), some("\x1b[13~"));
        assert_eq!(sent("shift-f3", KITTY), some("\x1b[13;2~"));
    }

    #[test]
    fn kitty_keeps_the_keys_a_shell_needs() {
        assert_eq!(sent("enter", KITTY), some("\r"));
        assert_eq!(sent("tab", KITTY), some("\t"));
        assert_eq!(sent("backspace", KITTY), some("\x7f"));
        assert_eq!(sent("space", KITTY), some(" "));
        assert_eq!(sent("up", KITTY), some("\x1b[A"));
        // The protocol ignores application cursor mode.
        let both = KeyMode {
            app_cursor: true,
            ..KITTY
        };
        assert_eq!(sent("up", both), some("\x1b[A"));
        assert_eq!(sent("f1", both), some("\x1bOP"));
    }

    #[test]
    fn kitty_all_keys_as_escape_codes() {
        let all = KeyMode {
            kitty_all_keys: true,
            ..KITTY
        };
        assert_eq!(sent("enter", all), some("\x1b[13u"));
        assert_eq!(sent("tab", all), some("\x1b[9u"));
        assert_eq!(sent("backspace", all), some("\x1b[127u"));
    }

    #[test]
    fn which_keys_the_terminal_claims() {
        let claimed = |source: &'static str| is_claimed(&Keystroke::parse(source).unwrap());
        assert!(claimed("tab"));
        assert!(claimed("escape"));
        assert!(claimed("shift-enter"));
        assert!(claimed("ctrl-c"));
        assert!(claimed("alt-b"));
        assert!(claimed("alt-left"));
        assert!(!claimed("cmd-c"));
        assert!(!claimed("ctrl-tab"));
        assert!(!claimed("ctrl-shift-c"));
        assert!(!claimed("ctrl-alt-x"));
    }
}
