/// Plain text buffer over char vectors with the primitives the engine needs.
/// Buffers here are titles and notes — small — so simplicity beats ropes.
pub type Pos = (usize, usize); // (line, char-column)

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buffer {
    pub lines: Vec<Vec<char>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    Whitespace,
    Word, // alphanumeric or '_' (or any non-space for big words)
    Punct,
}

impl Buffer {
    pub fn from_text(text: &str) -> Buffer {
        let lines: Vec<Vec<char>> = if text.is_empty() {
            vec![Vec::new()]
        } else {
            text.split('\n').map(|l| l.chars().collect()).collect()
        };
        Buffer { lines }
    }

    pub fn to_text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line_len(&self, line: usize) -> usize {
        self.lines[line].len()
    }

    /// Last valid (line, col) for normal-mode cursor.
    pub fn last_pos(&self) -> Pos {
        let line = self.lines.len() - 1;
        (line, self.line_len(line).saturating_sub(1))
    }

    pub fn char_at(&self, pos: Pos) -> Option<char> {
        self.lines.get(pos.0).and_then(|l| l.get(pos.1)).copied()
    }

    pub fn clamp_normal(&self, pos: Pos) -> Pos {
        let line = pos.0.min(self.lines.len() - 1);
        let len = self.line_len(line);
        let col = if len == 0 { 0 } else { pos.1.min(len - 1) };
        (line, col)
    }

    pub fn first_non_blank(&self, line: usize) -> usize {
        let l = &self.lines[line];
        l.iter()
            .position(|c| !c.is_whitespace())
            .unwrap_or(l.len().saturating_sub(1).min(l.len()))
    }

    pub fn insert_char(&mut self, pos: Pos, c: char) {
        let line = &mut self.lines[pos.0];
        let col = pos.1.min(line.len());
        line.insert(col, c);
    }

    /// Remove the charwise range [start, end) (positions ordered by the
    /// buffer's ordering). Returns the removed text.
    pub fn remove_charwise(&mut self, start: Pos, end: Pos) -> String {
        let (start, end) = self.order(start, end);
        let (l1, c1) = start;
        let (l2, c2) = end;
        if l1 == l2 {
            let line = &mut self.lines[l1];
            let removed: String = line.drain(c1..c2.min(line.len())).collect();
            return removed;
        }
        // Multi-line: stitch head + tail.
        let tail: Vec<char> = self.lines[l2][c2.min(self.lines[l2].len())..].to_vec();
        let mut removed = String::new();
        removed.push_str(&self.lines[l1][c1..].iter().collect::<String>());
        removed.push('\n');
        for l in &self.lines[l1 + 1..l2] {
            removed.push_str(&l.iter().collect::<String>());
            removed.push('\n');
        }
        removed.push_str(
            &self.lines[l2][..c2.min(self.lines[l2].len())]
                .iter()
                .collect::<String>(),
        );
        let mut merged: Vec<char> = self.lines[l1][..c1].to_vec();
        merged.extend(tail);
        self.lines.splice(l1..=l2, std::iter::once(merged));
        removed
    }

    /// Remove whole lines [from, to]. Returns text with trailing '\n'.
    pub fn remove_lines(&mut self, from: usize, to: usize) -> String {
        let from = from.min(self.lines.len() - 1);
        let to = to.min(self.lines.len() - 1);
        let (from, to) = (from.min(to), from.max(to));
        let removed: Vec<String> = self.lines[from..=to]
            .iter()
            .map(|l| l.iter().collect())
            .collect();
        if self.lines.len() == to - from + 1 {
            // Never leave the buffer with zero lines.
            self.lines = vec![Vec::new()];
        } else {
            self.lines.drain(from..=to);
        }
        let mut text = removed.join("\n");
        text.push('\n');
        text
    }

    /// Insert charwise text at pos (may contain newlines). Returns the
    /// position of the last inserted char.
    pub fn insert_text(&mut self, pos: Pos, text: &str) -> Pos {
        let (line, col) = (pos.0.min(self.lines.len() - 1), pos.1);
        let parts: Vec<&str> = text.split('\n').collect();
        if parts.len() == 1 {
            let l = &mut self.lines[line];
            let col = col.min(l.len());
            l.splice(col..col, parts[0].chars());
            return (line, col + parts[0].chars().count().saturating_sub(1));
        }
        let l = &mut self.lines[line];
        let col = col.min(l.len());
        let before: Vec<char> = l[..col].to_vec();
        let after: Vec<char> = l[col..].to_vec();
        let mut new_lines: Vec<Vec<char>> = vec![before];
        new_lines.last_mut().unwrap().extend(parts[0].chars());
        for mid in &parts[1..parts.len() - 1] {
            new_lines.push(mid.chars().collect());
        }
        let mut last: Vec<char> = parts[parts.len() - 1].chars().collect();
        let last_len = last.len();
        last.extend(after);
        new_lines.push(last);
        self.lines.splice(line..=line, new_lines);
        (line + parts.len() - 1, last_len.saturating_sub(1))
    }

    /// Insert whole lines before the given line index (0-based). `before`
    /// may be lines.len() to append at the end.
    pub fn insert_lines(&mut self, before: usize, lines_text: &str) {
        let new_lines: Vec<Vec<char>> = lines_text
            .trim_end_matches('\n')
            .split('\n')
            .map(|l| l.chars().collect())
            .collect();
        let at = before.min(self.lines.len());
        self.lines.splice(at..at, new_lines);
    }

    pub fn order(&self, a: Pos, b: Pos) -> (Pos, Pos) {
        if (a.0, a.1) <= (b.0, b.1) {
            (a, b)
        } else {
            (b, a)
        }
    }

    pub fn pos_after(&self, pos: Pos) -> Pos {
        // Next position in char order (skipping nothing; EOL → next line col 0).
        if pos.1 < self.line_len(pos.0) {
            (pos.0, pos.1 + 1)
        } else if pos.0 + 1 < self.lines.len() {
            (pos.0 + 1, 0)
        } else {
            pos
        }
    }

    pub fn pos_before(&self, pos: Pos) -> Pos {
        if pos.1 > 0 {
            (pos.0, pos.1 - 1)
        } else if pos.0 > 0 {
            (pos.0 - 1, self.line_len(pos.0 - 1))
        } else {
            pos
        }
    }
}

pub fn char_class(c: char, big: bool) -> CharClass {
    if c.is_whitespace() {
        CharClass::Whitespace
    } else if big || c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}
