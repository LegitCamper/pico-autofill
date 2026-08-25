pub const LEFT_SHIFT: u8 = 0x02;

pub fn ascii_to_hid(byte: u8) -> Option<(u8, u8)> {
    let result = match byte {
        b'a'..=b'z' => (0, 4 + byte - b'a'),
        b'A'..=b'Z' => (LEFT_SHIFT, 4 + byte - b'A'),
        b'1'..=b'9' => (0, 30 + byte - b'1'),
        b'0' => (0, 39),
        b' ' => (0, 44),
        b'-' => (0, 45),
        b'_' => (LEFT_SHIFT, 45),
        b'=' => (0, 46),
        b'+' => (LEFT_SHIFT, 46),
        b'[' => (0, 47),
        b'{' => (LEFT_SHIFT, 47),
        b']' => (0, 48),
        b'}' => (LEFT_SHIFT, 48),
        b'\\' => (0, 49),
        b'|' => (LEFT_SHIFT, 49),
        b';' => (0, 51),
        b':' => (LEFT_SHIFT, 51),
        b'\'' => (0, 52),
        b'"' => (LEFT_SHIFT, 52),
        b'`' => (0, 53),
        b'~' => (LEFT_SHIFT, 53),
        b',' => (0, 54),
        b'<' => (LEFT_SHIFT, 54),
        b'.' => (0, 55),
        b'>' => (LEFT_SHIFT, 55),
        b'/' => (0, 56),
        b'?' => (LEFT_SHIFT, 56),
        b'!' => (LEFT_SHIFT, 30),
        b'@' => (LEFT_SHIFT, 31),
        b'#' => (LEFT_SHIFT, 32),
        b'$' => (LEFT_SHIFT, 33),
        b'%' => (LEFT_SHIFT, 34),
        b'^' => (LEFT_SHIFT, 35),
        b'&' => (LEFT_SHIFT, 36),
        b'*' => (LEFT_SHIFT, 37),
        b'(' => (LEFT_SHIFT, 38),
        b')' => (LEFT_SHIFT, 39),
        _ => return None,
    };
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_should_map_to_us_keyboard_usages() {
        assert_eq!(ascii_to_hid(b'a'), Some((0, 4)));
        assert_eq!(ascii_to_hid(b'Z'), Some((LEFT_SHIFT, 29)));
    }

    #[test]
    fn punctuation_should_include_shift_modifier() {
        assert_eq!(ascii_to_hid(b'?'), Some((LEFT_SHIFT, 56)));
        assert_eq!(ascii_to_hid(b'/'), Some((0, 56)));
    }
}
