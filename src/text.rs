pub const MAX_TEXT_LEN: usize = 2_500;
const _: () = assert!(MAX_TEXT_LEN <= u16::MAX as usize);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutofillText {
    bytes: [u8; MAX_TEXT_LEN],
    len: u16,
}

impl AutofillText {
    pub const fn empty() -> Self {
        Self {
            bytes: [0; MAX_TEXT_LEN],
            len: 0,
        }
    }

    pub fn from_file_bytes(input: &[u8]) -> Self {
        let logical_end = input
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(input.len());
        let mut end = logical_end;
        while end > 0 && matches!(input[end - 1], b'\r' | b'\n') {
            end -= 1;
        }

        let mut text = Self::empty();
        for &byte in &input[..end] {
            if (b' '..=b'~').contains(&byte) {
                if usize::from(text.len) == MAX_TEXT_LEN {
                    break;
                }
                text.bytes[usize::from(text.len)] = byte;
                text.len += 1;
            }
        }
        text
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for AutofillText {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_should_trim_editor_line_endings() {
        let text = AutofillText::from_file_bytes(b"correct horse\r\n");
        assert_eq!(text.as_bytes(), b"correct horse");
    }

    #[test]
    fn text_should_truncate_at_configured_limit() {
        let input = [b'x'; MAX_TEXT_LEN + 1];
        let text = AutofillText::from_file_bytes(&input);
        assert_eq!(text.len(), MAX_TEXT_LEN);
    }

    #[test]
    fn text_should_drop_unsupported_bytes() {
        let text = AutofillText::from_file_bytes(b"a\t\xffb");
        assert_eq!(text.as_bytes(), b"ab");
    }
}
