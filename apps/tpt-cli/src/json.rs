//! Minimal, dependency-free JSON writer.
//!
//! Deterministic field order, correct string escaping, no float printing
//! beyond `u64`/`i64`/`f64` (`f64` emitted via Rust's `{}`, which is
//! guaranteed round-trippable in JSON).

/// Tracks the container stack so separators are emitted automatically.
pub struct J {
    buf: String,
    /// `false` = object, `true` = array, one entry per open container.
    stack: Vec<bool>,
    /// `true` when the current container has not yet received an element.
    first: Vec<bool>,
}

impl J {
    /// A fresh writer.
    pub fn new() -> Self {
        J {
            buf: String::new(),
            stack: Vec::new(),
            first: Vec::new(),
        }
    }

    fn sep(&mut self) {
        if let Some(&f) = self.first.last() {
            if !f {
                self.buf.push(',');
            } else if let Some(slot) = self.first.last_mut() {
                *slot = false;
            }
        }
    }

    /// Open an object (`{`).
    ///
    /// Emits no separator of its own: a nested container is either the
    /// value of a [`field`](Self::field) (which already separated) or an
    /// array element (separated by [`elem`](Self::elem)). `object()` used
    /// to call `sep()` as well, producing invalid JSON like
    /// `"key":,{...}` — see
    /// `json_nested_containers_do_not_emit_stray_commas`.
    pub fn object(&mut self) {
        self.buf.push('{');
        self.stack.push(false);
        self.first.push(true);
    }

    /// Open an array (`[`); no separator, for the same reason as
    /// [`object`](Self::object).
    pub fn arr(&mut self) {
        self.buf.push('[');
        self.stack.push(true);
        self.first.push(true);
    }

    /// Close the current object/array.
    ///
    /// The bracket is chosen by the container being closed, not its
    /// parent: `close()` used to inspect the *parent* kind, so an object
    /// nested inside an array closed with `]` (invalid JSON like
    /// `[{...]`) — exactly the shape every probe track list emits. See
    /// `json_nested_containers_do_not_emit_stray_commas`.
    pub fn close(&mut self) {
        let was_array = self
            .stack
            .pop()
            .expect("close() without a matching object()/arr()");
        self.first.pop();
        self.buf.push(if was_array { ']' } else { '}' });
    }

    /// Begin an object field `"key":`.
    pub fn field(&mut self, key: &str) {
        self.sep();
        self.push_string(key);
        self.buf.push(':');
    }

    /// Mark another array element (used before scalars/ranges in arrays).
    pub fn elem(&mut self) {
        self.sep();
    }

    fn push_string(&mut self, s: &str) {
        self.buf.push('"');
        for c in s.chars() {
            match c {
                '"' => self.buf.push_str("\\\""),
                '\\' => self.buf.push_str("\\\\"),
                '\n' => self.buf.push_str("\\n"),
                '\r' => self.buf.push_str("\\r"),
                '\t' => self.buf.push_str("\\t"),
                c if (c as u32) < 0x20 => {
                    self.buf.push_str(&format!("\\u{:04x}", c as u32));
                }
                c => self.buf.push(c),
            }
        }
        self.buf.push('"');
    }

    /// Emit a string value.
    pub fn value_str(&mut self, s: &str) {
        self.push_string(s);
    }

    /// Emit an integer value.
    pub fn value_u64(&mut self, v: u64) {
        self.buf.push_str(&v.to_string());
    }

    /// Emit a signed integer value.
    pub fn value_i64(&mut self, v: i64) {
        self.buf.push_str(&v.to_string());
    }

    /// Emit a float value.
    pub fn value_f64(&mut self, v: f64) {
        self.buf.push_str(&format!("{v}"));
    }

    /// Emit a boolean value.
    pub fn value_bool(&mut self, v: bool) {
        self.buf.push_str(if v { "true" } else { "false" });
    }

    /// Emit `null`.
    pub fn value_null(&mut self) {
        self.buf.push_str("null");
    }

    /// Finish and return the JSON text.
    pub fn finish(self) -> String {
        self.buf
    }
}

impl Default for J {
    fn default() -> Self {
        J::new()
    }
}
