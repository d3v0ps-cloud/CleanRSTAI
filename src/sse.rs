//! Minimal incremental Server-Sent Events parser, shared by the LLM client and the
//! MCP Streamable HTTP transport.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    /// How much of `buf` is known to hold no line end, so a long line that arrives in
    /// many chunks is searched once, not once per chunk.
    scanned: usize,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    /// Feed raw bytes; returns every event completed by them.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(found) = self.buf[self.scanned..].iter().position(|&b| b == b'\n') {
            let end = self.scanned + found;
            let mut line = &self.buf[start..end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            let line = String::from_utf8_lossy(line).into_owned();
            self.process_line(&line, &mut out);
            start = end + 1;
            self.scanned = start;
        }
        // The lines read are dropped in one move.
        self.buf.drain(..start);
        self.scanned = self.buf.len();
        out
    }

    /// Flush a trailing event that wasn't terminated by a blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        self.scanned = 0;
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned();
            let mut out = Vec::new();
            self.process_line(line.trim_end_matches('\r'), &mut out);
            if let Some(ev) = out.pop() {
                return Some(ev);
            }
        }
        self.dispatch()
    }

    fn process_line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if let Some(ev) = self.dispatch() {
                out.push(ev);
            }
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "data" => self.data.push(value.to_owned()),
            "event" => self.event = Some(value.to_owned()),
            _ => {}
        }
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let event = self.event.take();
        if self.data.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.data).join("\n");
        Some(SseEvent { event, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// However a stream is split into chunks, the same events come out; random bytes never
    /// panic.
    #[test]
    fn chunk_boundaries_dont_matter() {
        let stream = "event: a\ndata: one\r\n\n: comment\ndata: two\ndata: lines\n\n\
                      data:{\"x\":\"é…\"}\n\nretry: 5\ndata\n\nevent: tail\ndata: end";
        let whole = {
            let mut p = SseParser::default();
            let mut out = p.feed(stream.as_bytes());
            out.extend(p.finish());
            out
        };
        assert_eq!(whole.len(), 5);
        let mut rng = crate::documents::tests::Mutator::new(7);
        let bytes = stream.as_bytes();
        for _ in 0..500 {
            let mut p = SseParser::default();
            let mut out = Vec::new();
            let mut at = 0;
            while at < bytes.len() {
                let n = 1 + (rng.next() % 9) as usize;
                let end = (at + n).min(bytes.len());
                out.extend(p.feed(&bytes[at..end]));
                at = end;
            }
            out.extend(p.finish());
            assert_eq!(out, whole);
        }
        for _ in 0..200 {
            let junk: Vec<u8> = (0..(rng.next() % 300)).map(|_| rng.next() as u8).collect();
            let mut p = SseParser::default();
            p.feed(&junk);
            p.finish();
        }
    }

    #[test]
    fn parses_split_chunks_and_crlf() {
        let mut p = SseParser::default();
        let mut events = p.feed(b"data: {\"a\"");
        assert!(events.is_empty());
        events.extend(p.feed(b":1}\r\n\r\n: comment\nevent: message\ndata: x\ndata: y\n\n"));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"a\":1}");
        assert_eq!(events[1].event.as_deref(), Some("message"));
        assert_eq!(events[1].data, "x\ny");
    }

    #[test]
    fn utf8_split_across_chunks() {
        let mut p = SseParser::default();
        let bytes = "data: héllo\n\n".as_bytes();
        let (a, b) = bytes.split_at(8); // splits inside 'é'
        assert!(p.feed(a).is_empty());
        assert_eq!(p.feed(b)[0].data, "héllo");
    }

    #[test]
    fn finish_flushes_unterminated() {
        let mut p = SseParser::default();
        assert!(p.feed(b"data: [DONE]").is_empty());
        assert_eq!(p.finish().unwrap().data, "[DONE]");
    }

    #[test]
    fn a_long_line_in_small_chunks_is_read_once() {
        let mut p = SseParser::default();
        let line = format!("data: {}\n\ndata: next\r\n\r\n", "x".repeat(4 << 20));
        let started = std::time::Instant::now();
        let events: Vec<SseEvent> = line
            .as_bytes()
            .chunks(512)
            .flat_map(|c| p.feed(c))
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data.len(), 4 << 20);
        assert_eq!(events[1].data, "next");
        assert!(p.finish().is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
