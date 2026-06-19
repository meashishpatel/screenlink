//! Mapping between the portable [`Key`] model and Windows virtual-key codes.
//!
//! `Key` names physical positions (US layout), so this table is what makes a key
//! pressed on any OS land on the right Windows key and vice-versa.

use screenlink_core::protocol::Key;

/// How to inject a [`Key`] on Windows.
pub enum VkMapping {
    /// A virtual-key code and whether it needs the extended-key flag.
    Vk(u16, bool),
    /// Type a literal character via the Unicode injection path.
    Unicode(char),
}

/// Map a portable key to its Windows injection form.
pub fn vk_for(key: Key) -> VkMapping {
    use Key::*;
    let (vk, ext): (u16, bool) = match key {
        // Letters 'A'..'Z' == 0x41..0x5A.
        A => (0x41, false),
        B => (0x42, false),
        C => (0x43, false),
        D => (0x44, false),
        E => (0x45, false),
        F => (0x46, false),
        G => (0x47, false),
        H => (0x48, false),
        I => (0x49, false),
        J => (0x4A, false),
        K => (0x4B, false),
        L => (0x4C, false),
        M => (0x4D, false),
        N => (0x4E, false),
        O => (0x4F, false),
        P => (0x50, false),
        Q => (0x51, false),
        R => (0x52, false),
        S => (0x53, false),
        T => (0x54, false),
        U => (0x55, false),
        V => (0x56, false),
        W => (0x57, false),
        X => (0x58, false),
        Y => (0x59, false),
        Z => (0x5A, false),
        // Number row '0'..'9' == 0x30..0x39.
        Num0 => (0x30, false),
        Num1 => (0x31, false),
        Num2 => (0x32, false),
        Num3 => (0x33, false),
        Num4 => (0x34, false),
        Num5 => (0x35, false),
        Num6 => (0x36, false),
        Num7 => (0x37, false),
        Num8 => (0x38, false),
        Num9 => (0x39, false),
        // Function keys VK_F1..VK_F12 == 0x70..0x7B.
        F1 => (0x70, false),
        F2 => (0x71, false),
        F3 => (0x72, false),
        F4 => (0x73, false),
        F5 => (0x74, false),
        F6 => (0x75, false),
        F7 => (0x76, false),
        F8 => (0x77, false),
        F9 => (0x78, false),
        F10 => (0x79, false),
        F11 => (0x7A, false),
        F12 => (0x7B, false),
        // Modifiers.
        ControlLeft => (0xA2, false),
        ControlRight => (0xA3, true),
        ShiftLeft => (0xA0, false),
        ShiftRight => (0xA1, false),
        AltLeft => (0xA4, false),
        AltRight => (0xA5, true),
        MetaLeft => (0x5B, true),
        MetaRight => (0x5C, true),
        // Editing / whitespace.
        Enter => (0x0D, false),
        Escape => (0x1B, false),
        Backspace => (0x08, false),
        Tab => (0x09, false),
        Space => (0x20, false),
        CapsLock => (0x14, false),
        // Navigation (all extended on Windows).
        Insert => (0x2D, true),
        Delete => (0x2E, true),
        Home => (0x24, true),
        End => (0x23, true),
        PageUp => (0x21, true),
        PageDown => (0x22, true),
        ArrowUp => (0x26, true),
        ArrowDown => (0x28, true),
        ArrowLeft => (0x25, true),
        ArrowRight => (0x27, true),
        // Punctuation (OEM keys).
        Minus => (0xBD, false),
        Equal => (0xBB, false),
        BracketLeft => (0xDB, false),
        BracketRight => (0xDD, false),
        Backslash => (0xDC, false),
        Semicolon => (0xBA, false),
        Quote => (0xDE, false),
        Backquote => (0xC0, false),
        Comma => (0xBC, false),
        Period => (0xBE, false),
        Slash => (0xBF, false),
        // Numpad.
        Numpad0 => (0x60, false),
        Numpad1 => (0x61, false),
        Numpad2 => (0x62, false),
        Numpad3 => (0x63, false),
        Numpad4 => (0x64, false),
        Numpad5 => (0x65, false),
        Numpad6 => (0x66, false),
        Numpad7 => (0x67, false),
        Numpad8 => (0x68, false),
        Numpad9 => (0x69, false),
        NumpadAdd => (0x6B, false),
        NumpadSubtract => (0x6D, false),
        NumpadMultiply => (0x6A, false),
        NumpadDivide => (0x6F, true),
        NumpadDecimal => (0x6E, false),
        NumpadEnter => (0x0D, true),
        // System / locks.
        PrintScreen => (0x2C, false),
        ScrollLock => (0x91, false),
        Pause => (0x13, false),
        NumLock => (0x90, true),
        ContextMenu => (0x5D, true),
        // Fallbacks.
        Char(c) => return VkMapping::Unicode(c),
        Raw(n) => (n as u16, false),
    };
    VkMapping::Vk(vk, ext)
}

/// Map a Windows virtual-key code (from a hook) to a portable key.
pub fn key_for_vk(vk: u16, extended: bool) -> Key {
    use Key::*;
    match vk {
        0x41 => A,
        0x42 => B,
        0x43 => C,
        0x44 => D,
        0x45 => E,
        0x46 => F,
        0x47 => G,
        0x48 => H,
        0x49 => I,
        0x4A => J,
        0x4B => K,
        0x4C => L,
        0x4D => M,
        0x4E => N,
        0x4F => O,
        0x50 => P,
        0x51 => Q,
        0x52 => R,
        0x53 => S,
        0x54 => T,
        0x55 => U,
        0x56 => V,
        0x57 => W,
        0x58 => X,
        0x59 => Y,
        0x5A => Z,
        0x30 => Num0,
        0x31 => Num1,
        0x32 => Num2,
        0x33 => Num3,
        0x34 => Num4,
        0x35 => Num5,
        0x36 => Num6,
        0x37 => Num7,
        0x38 => Num8,
        0x39 => Num9,
        0x70 => F1,
        0x71 => F2,
        0x72 => F3,
        0x73 => F4,
        0x74 => F5,
        0x75 => F6,
        0x76 => F7,
        0x77 => F8,
        0x78 => F9,
        0x79 => F10,
        0x7A => F11,
        0x7B => F12,
        0x11 | 0xA2 => ControlLeft,
        0xA3 => ControlRight,
        0x10 | 0xA0 => ShiftLeft,
        0xA1 => ShiftRight,
        0x12 | 0xA4 => AltLeft,
        0xA5 => AltRight,
        0x5B => MetaLeft,
        0x5C => MetaRight,
        0x0D => {
            if extended {
                NumpadEnter
            } else {
                Enter
            }
        }
        0x1B => Escape,
        0x08 => Backspace,
        0x09 => Tab,
        0x20 => Space,
        0x14 => CapsLock,
        0x2D => Insert,
        0x2E => Delete,
        0x24 => Home,
        0x23 => End,
        0x21 => PageUp,
        0x22 => PageDown,
        0x26 => ArrowUp,
        0x28 => ArrowDown,
        0x25 => ArrowLeft,
        0x27 => ArrowRight,
        0xBD => Minus,
        0xBB => Equal,
        0xDB => BracketLeft,
        0xDD => BracketRight,
        0xDC => Backslash,
        0xBA => Semicolon,
        0xDE => Quote,
        0xC0 => Backquote,
        0xBC => Comma,
        0xBE => Period,
        0xBF => Slash,
        0x60 => Numpad0,
        0x61 => Numpad1,
        0x62 => Numpad2,
        0x63 => Numpad3,
        0x64 => Numpad4,
        0x65 => Numpad5,
        0x66 => Numpad6,
        0x67 => Numpad7,
        0x68 => Numpad8,
        0x69 => Numpad9,
        0x6B => NumpadAdd,
        0x6D => NumpadSubtract,
        0x6A => NumpadMultiply,
        0x6F => NumpadDivide,
        0x6E => NumpadDecimal,
        0x2C => PrintScreen,
        0x91 => ScrollLock,
        0x13 => Pause,
        0x90 => NumLock,
        0x5D => ContextMenu,
        other => Raw(other as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_roundtrip_through_vk() {
        for key in [Key::A, Key::Z, Key::Num5, Key::F7, Key::Home, Key::Slash] {
            if let VkMapping::Vk(vk, ext) = vk_for(key) {
                assert_eq!(key_for_vk(vk, ext), key, "roundtrip failed for {key:?}");
            } else {
                panic!("expected Vk mapping for {key:?}");
            }
        }
    }

    #[test]
    fn char_maps_to_unicode_path() {
        assert!(matches!(vk_for(Key::Char('é')), VkMapping::Unicode('é')));
    }

    #[test]
    fn unknown_vk_falls_back_to_raw() {
        assert_eq!(key_for_vk(0xFF, false), Key::Raw(0xFF));
    }
}
