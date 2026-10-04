use std::collections::HashMap;

pub(crate) struct JsonIndex {
    offsets: HashMap<String, usize>,
    line_starts: Vec<usize>,
}

impl JsonIndex {
    pub(crate) fn new(text: &str) -> Self {
        let mut scanner = Scanner {
            bytes: text.as_bytes(),
            pos: 0,
            offsets: HashMap::new(),
        };
        scanner.skip_whitespace();
        scanner.offsets.insert(String::new(), scanner.pos);
        scanner.index_value(&mut String::new());
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(newline, _)| newline + 1))
            .collect();
        Self {
            offsets: scanner.offsets,
            line_starts,
        }
    }

    pub(crate) fn line(&self, pointer: &str) -> Option<usize> {
        let offset = *self.offsets.get(pointer)?;
        Some(self.line_starts.partition_point(|&start| start <= offset))
    }

    pub(crate) fn nearest_line(&self, pointer: &str) -> Option<usize> {
        std::iter::successors(Some(pointer), |pointer| {
            pointer.rfind('/').map(|end| &pointer[..end])
        })
        .find_map(|pointer| self.line(pointer))
    }
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
    offsets: HashMap<String, usize>,
}

impl<'a> Scanner<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn consume(&mut self, expected: u8) -> Option<()> {
        self.skip_whitespace();
        (self.peek()? == expected).then(|| self.pos += 1)
    }

    fn index_value(&mut self, pointer: &mut String) -> Option<()> {
        match self.peek()? {
            b'{' => self.index_members(pointer),
            b'[' => self.index_elements(pointer),
            b'"' => self.string().map(|_| ()),
            _ => {
                while self
                    .peek()
                    .is_some_and(|byte| !b",}] \t\r\n".contains(&byte))
                {
                    self.pos += 1;
                }
                Some(())
            }
        }
    }

    fn index_members(&mut self, pointer: &mut String) -> Option<()> {
        self.pos += 1;
        self.skip_whitespace();
        if self.peek()? == b'}' {
            self.pos += 1;
            return Some(());
        }
        loop {
            self.skip_whitespace();
            let key_start = self.pos;
            let key: String = serde_json::from_slice(self.string()?).ok()?;
            self.consume(b':')?;
            self.skip_whitespace();
            let parent_len = pointer.len();
            pointer.push('/');
            pointer.push_str(&key.replace('~', "~0").replace('/', "~1"));
            self.offsets.insert(pointer.clone(), key_start);
            let indexed = self.index_value(pointer);
            pointer.truncate(parent_len);
            indexed?;
            self.skip_whitespace();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Some(());
                }
                _ => return None,
            }
        }
    }

    fn index_elements(&mut self, pointer: &mut String) -> Option<()> {
        self.pos += 1;
        self.skip_whitespace();
        if self.peek()? == b']' {
            self.pos += 1;
            return Some(());
        }
        for index in 0.. {
            self.skip_whitespace();
            let parent_len = pointer.len();
            pointer.push('/');
            pointer.push_str(&index.to_string());
            self.offsets.insert(pointer.clone(), self.pos);
            let indexed = self.index_value(pointer);
            pointer.truncate(parent_len);
            indexed?;
            self.skip_whitespace();
            match self.peek()? {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Some(());
                }
                _ => return None,
            }
        }
        None
    }

    fn string(&mut self) -> Option<&'a [u8]> {
        let start = self.pos;
        if self.peek()? != b'"' {
            return None;
        }
        self.pos += 1;
        loop {
            match self.peek()? {
                b'\\' => self.pos += 2,
                b'"' => {
                    self.pos += 1;
                    return Some(&self.bytes[start..self.pos]);
                }
                _ => self.pos += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &str = r#"{
  "profiles": [
    { "name": "a\"}]", "port": 1 },
    {
      "name": "b",
      "port": 0
    }
  ],
  "a/b~c": { "nested": [true, null, {"x": -1.5e3}] },
  "settings": {}
}"#;

    #[test]
    fn locates_member_keys_and_array_elements() {
        let index = JsonIndex::new(DOCUMENT);
        assert_eq!(index.line(""), Some(1));
        assert_eq!(index.line("/profiles"), Some(2));
        assert_eq!(index.line("/profiles/0/port"), Some(3));
        assert_eq!(index.line("/profiles/1"), Some(4));
        assert_eq!(index.line("/profiles/1/port"), Some(6));
        assert_eq!(index.line("/a~1b~0c/nested/2/x"), Some(9));
    }

    #[test]
    fn missing_targets_are_not_located() {
        let index = JsonIndex::new(DOCUMENT);
        for pointer in [
            "/profiles/2",
            "/profiles/x",
            "/profiles/0/port/deeper",
            "/settings/theme",
            "no-leading-slash",
        ] {
            assert_eq!(index.line(pointer), None, "{pointer}");
        }
        assert_eq!(JsonIndex::new("{\"a\": ").line("/a/b"), None);
    }

    #[test]
    fn nearest_line_falls_back_to_the_closest_present_ancestor() {
        let index = JsonIndex::new(DOCUMENT);
        assert_eq!(index.nearest_line("/settings/theme"), Some(10));
        assert_eq!(index.nearest_line("/profiles/7/name"), Some(2));
        assert_eq!(index.nearest_line("/profiles/1/port"), Some(6));
    }
}
