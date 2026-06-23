//! Accumulate raw byte chunks, dispatch complete (newline-terminated) lines.
//!
//! Used by both the builtin-path dispatch hook (where bytes are written into
//! a Vec<u8> capture buffer) and the external poll loop (where bytes are
//! `read(2)` from a pipe).

#[derive(Default)]
#[allow(dead_code)]
pub struct LineBuf {
    partial: Vec<u8>,
}

#[allow(dead_code)]
impl LineBuf {
    pub fn new() -> Self {
        Self { partial: Vec::new() }
    }

    /// Append raw bytes. Caller pulls via `next_line()` after each push.
    pub fn push(&mut self, bytes: &[u8]) {
        self.partial.extend_from_slice(bytes);
    }

    /// Pull the next complete line (without trailing `\n`). Returns `None`
    /// when no more `\n` is present in the buffer.
    ///
    /// Decodes via `String::from_utf8_lossy` — invalid UTF-8 becomes U+FFFD,
    /// matching `Output.stdout` policy.
    pub fn next_line(&mut self) -> Option<String> {
        let pos = self.partial.iter().position(|&b| b == b'\n')?;
        let line_bytes: Vec<u8> = self.partial.drain(..=pos).collect();
        // line_bytes ends in \n; trim it.
        let trimmed = &line_bytes[..line_bytes.len() - 1];
        Some(String::from_utf8_lossy(trimmed).into_owned())
    }

    /// Pull whatever bytes remain (may be empty). For end-of-stream flush.
    /// Returns `None` if the buffer is empty (no final partial to deliver).
    pub fn drain_final(&mut self) -> Option<String> {
        if self.partial.is_empty() {
            return None;
        }
        let rest = std::mem::take(&mut self.partial);
        Some(String::from_utf8_lossy(&rest).into_owned())
    }

    /// Is the partial buffer empty? Used by debugging assertions.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.partial.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_complete_line() {
        let mut b = LineBuf::new();
        b.push(b"hello\n");
        assert_eq!(b.next_line().as_deref(), Some("hello"));
        assert_eq!(b.next_line(), None);
        assert!(b.is_empty());
    }

    #[test]
    fn multiple_lines_in_one_push() {
        let mut b = LineBuf::new();
        b.push(b"a\nb\nc\n");
        assert_eq!(b.next_line().as_deref(), Some("a"));
        assert_eq!(b.next_line().as_deref(), Some("b"));
        assert_eq!(b.next_line().as_deref(), Some("c"));
        assert_eq!(b.next_line(), None);
    }

    #[test]
    fn split_line_across_pushes() {
        let mut b = LineBuf::new();
        b.push(b"hel");
        assert_eq!(b.next_line(), None);
        b.push(b"lo\n");
        assert_eq!(b.next_line().as_deref(), Some("hello"));
    }

    #[test]
    fn empty_line() {
        let mut b = LineBuf::new();
        b.push(b"\n");
        assert_eq!(b.next_line().as_deref(), Some(""));
        assert_eq!(b.next_line(), None);
    }

    #[test]
    fn drain_final_partial() {
        let mut b = LineBuf::new();
        b.push(b"trailing");
        assert_eq!(b.next_line(), None);
        assert_eq!(b.drain_final().as_deref(), Some("trailing"));
        assert_eq!(b.drain_final(), None);
    }

    #[test]
    fn drain_final_after_complete_line_is_empty() {
        let mut b = LineBuf::new();
        b.push(b"hi\n");
        let _ = b.next_line();
        assert_eq!(b.drain_final(), None);
    }

    #[test]
    fn invalid_utf8_decoded_lossy() {
        let mut b = LineBuf::new();
        b.push(&[0xff, 0xfe, b'\n']);
        let line = b.next_line().unwrap();
        // U+FFFD is the replacement character, 3 bytes in UTF-8.
        assert!(line.contains('\u{FFFD}'));
    }
}
