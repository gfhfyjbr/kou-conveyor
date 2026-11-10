#[derive(Debug)]
pub(crate) struct BoundedOutput {
    limit: usize,
    head: Vec<u8>,
    tail: Vec<u8>,
    total: u64,
}

impl BoundedOutput {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(16),
            head: Vec::new(),
            tail: Vec::new(),
            total: 0,
        }
    }

    pub(crate) fn write(&mut self, mut bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        let take = (self.limit - self.head.len()).min(bytes.len());
        self.head.extend_from_slice(&bytes[..take]);
        bytes = &bytes[take..];
        if bytes.len() >= self.limit {
            self.tail.clear();
            self.tail
                .extend_from_slice(&bytes[bytes.len() - self.limit..]);
        } else {
            let discard = (self.tail.len() + bytes.len()).saturating_sub(self.limit);
            self.tail.drain(..discard);
            self.tail.extend_from_slice(bytes);
        }
    }

    pub(crate) fn render(&self) -> String {
        if self.total <= self.head.len() as u64 {
            return String::from_utf8_lossy(&self.head).into_owned();
        }
        let mut head = &self.head[..self.head.len().min(self.limit / 2)];
        let tail_length = self.tail.len().min(self.limit - self.limit / 2);
        let mut tail = &self.tail[self.tail.len() - tail_length..];
        while !head.is_empty() && std::str::from_utf8(head).is_err() {
            head = &head[..head.len() - 1]
        }
        while !tail.is_empty() && tail[0] & 0xc0 == 0x80 {
            tail = &tail[1..]
        }
        let skipped = self.total - head.len() as u64 - tail.len() as u64;
        format!(
            "{}...{skipped} bytes truncated...{}",
            String::from_utf8_lossy(head),
            String::from_utf8_lossy(tail)
        )
    }
}

pub(crate) fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut head = limit * 2 / 3;
    let mut tail = text.len() - limit / 3;
    while head > 0 && !text.is_char_boundary(head) {
        head -= 1
    }
    while tail < text.len() && !text.is_char_boundary(tail) {
        tail += 1
    }
    format!(
        "{}\n[… {} bytes left out …]\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_chunks_keep_the_same_head_tail_and_unicode_as_one_write() {
        let text = format!("начало{}конец", "中".repeat(10000));
        let mut output = BoundedOutput::new(400);
        for chunk in text.as_bytes().chunks(7) {
            output.write(chunk)
        }
        let mut once = BoundedOutput::new(400);
        once.write(text.as_bytes());
        assert_eq!(output.render(), once.render());
        assert!(output.head.capacity() <= 800);
        assert!(output.tail.capacity() <= 800);
        assert!(output.render().contains("bytes truncated"));
        assert!(output.render().ends_with("конец"));
        assert!(!output.render().contains('\u{fffd}'));
    }
}
