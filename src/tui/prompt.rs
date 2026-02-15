use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

#[derive(Debug, Clone)]
struct VisualLine {
    row: usize,
    start_col: usize, // char offset in the logical row
    end_col: usize,   // char offset (exclusive) in the logical row
    text: String,
}

#[derive(Debug, Clone)]
pub(super) struct PromptEditor {
    placeholder: String,
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,        // char index within the row
    desired_x: Option<usize>, // visual x in cells for up/down
    scroll: usize,            // visual row scroll (soft-wrapped lines)
    last_wrap_w: usize,       // updated during render
}

impl PromptEditor {
    pub(super) fn from_text(text: &str, placeholder: &str) -> Self {
        let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            placeholder: placeholder.to_string(),
            lines,
            cursor_row: 0,
            cursor_col: 0,
            desired_x: None,
            scroll: 0,
            last_wrap_w: 80,
        }
    }

    pub(super) fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(|s| s.to_string()).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.desired_x = None;
        self.scroll = 0;
    }

    pub(super) fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub(super) fn handle_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char(c) => {
                if k.modifiers.contains(KeyModifiers::CONTROL)
                    || k.modifiers.contains(KeyModifiers::ALT)
                {
                    return;
                }
                self.insert_char(c);
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Enter => self.insert_newline(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Up => self.move_visual(-1),
            KeyCode::Down => self.move_visual(1),
            KeyCode::Home => {
                self.cursor_col = 0;
                self.desired_x = None;
            }
            KeyCode::End => {
                self.cursor_col = self.line_len_chars(self.cursor_row);
                self.desired_x = None;
            }
            _ => {}
        }
    }

    pub(super) fn render(&mut self, f: &mut ratatui::Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(block.clone(), area);
        let inner = block.inner(area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let wrap_w = inner.width as usize;
        self.last_wrap_w = wrap_w.max(1);
        let visual = self.visual_lines(self.last_wrap_w);

        // Keep cursor visible in the scrolled visual viewport.
        let (cur_vy, cur_vx) = self.cursor_visual_pos(&visual);
        if self.desired_x.is_none() {
            self.desired_x = Some(cur_vx);
        }
        if cur_vy < self.scroll {
            self.scroll = cur_vy;
        } else if cur_vy >= self.scroll + inner.height as usize {
            self.scroll = cur_vy.saturating_sub(inner.height as usize - 1);
        }

        let start = self.scroll.min(visual.len());
        let end = (start + inner.height as usize).min(visual.len());

        let mut out_lines: Vec<Line<'static>> = Vec::new();
        if (self.lines.len() == 1 && self.lines[0].is_empty()) || visual.is_empty() {
            out_lines.push(Line::from(Span::styled(
                self.placeholder.clone(),
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for vl in &visual[start..end] {
                // Pad to width for stable cursor placement.
                let mut t = vl.text.clone();
                let w = t.width();
                if w < self.last_wrap_w {
                    t.push_str(&" ".repeat(self.last_wrap_w - w));
                }
                out_lines.push(Line::from(t));
            }
        }

        let para = Paragraph::new(Text::from(out_lines)).style(Style::default());
        f.render_widget(para, inner);

        let cur_screen_y = cur_vy.saturating_sub(self.scroll);
        if cur_screen_y < inner.height as usize {
            let x = inner.x.saturating_add(cur_vx as u16);
            let y = inner.y.saturating_add(cur_screen_y as u16);
            f.set_cursor_position((x, y));
        }
    }

    fn insert_char(&mut self, c: char) {
        let row = self.cursor_row.min(self.lines.len().saturating_sub(1));
        let col = self.cursor_col.min(self.line_len_chars(row));
        let s = &mut self.lines[row];
        let byte = char_to_byte_index(s, col);
        s.insert(byte, c);
        self.cursor_row = row;
        self.cursor_col = col + 1;
        self.desired_x = None;
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let row = self.cursor_row;
            let col = self.cursor_col.min(self.line_len_chars(row));
            let s = &mut self.lines[row];
            let b0 = char_to_byte_index(s, col - 1);
            let b1 = char_to_byte_index(s, col);
            s.replace_range(b0..b1, "");
            self.cursor_col = col - 1;
            self.desired_x = None;
            return;
        }
        if self.cursor_row > 0 {
            let row = self.cursor_row;
            let cur = self.lines.remove(row);
            let prev = row - 1;
            let prev_len = self.line_len_chars(prev);
            self.lines[prev].push_str(&cur);
            self.cursor_row = prev;
            self.cursor_col = prev_len;
            self.desired_x = None;
        }
    }

    fn insert_newline(&mut self) {
        let row = self.cursor_row;
        let col = self.cursor_col.min(self.line_len_chars(row));
        let s = &mut self.lines[row];
        let byte = char_to_byte_index(s, col);
        let tail = s[byte..].to_string();
        s.truncate(byte);
        self.lines.insert(row + 1, tail);
        self.cursor_row = row + 1;
        self.cursor_col = 0;
        self.desired_x = None;
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            self.desired_x = None;
            return;
        }
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.line_len_chars(self.cursor_row);
            self.desired_x = None;
        }
    }

    fn move_right(&mut self) {
        let len = self.line_len_chars(self.cursor_row);
        if self.cursor_col < len {
            self.cursor_col += 1;
            self.desired_x = None;
            return;
        }
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
            self.desired_x = None;
        }
    }

    fn move_visual(&mut self, delta: i32) {
        let wrap_w = self.last_wrap_w.max(1);
        let visual = self.visual_lines(wrap_w);
        if visual.is_empty() {
            return;
        }
        let (cur_vy, cur_vx) = self.cursor_visual_pos(&visual);
        let vx = self.desired_x.unwrap_or(cur_vx);

        let target_vy = if delta < 0 {
            cur_vy.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (cur_vy + delta as usize).min(visual.len().saturating_sub(1))
        };

        let vl = &visual[target_vy];
        let row = vl.row;
        let col = col_at_x(&self.lines[row], vl.start_col, vl.end_col, vx);
        self.cursor_row = row;
        self.cursor_col = col;
        self.desired_x = Some(vx);
    }

    fn cursor_visual_pos(&self, visual: &[VisualLine]) -> (usize, usize) {
        if visual.is_empty() {
            return (0, 0);
        }
        let row = self.cursor_row.min(self.lines.len().saturating_sub(1));
        let col = self.cursor_col.min(self.line_len_chars(row));
        for (i, vl) in visual.iter().enumerate() {
            if vl.row != row {
                continue;
            }
            if col < vl.start_col {
                continue;
            }
            if col > vl.end_col {
                continue;
            }
            let seg = slice_chars(&self.lines[row], vl.start_col, col);
            return (i, seg.width().min(self.last_wrap_w));
        }
        (0, 0)
    }

    fn visual_lines(&self, wrap_w: usize) -> Vec<VisualLine> {
        if wrap_w == 0 {
            return vec![];
        }
        let mut out = Vec::new();
        for (row, line) in self.lines.iter().enumerate() {
            if line.is_empty() {
                out.push(VisualLine {
                    row,
                    start_col: 0,
                    end_col: 0,
                    text: String::new(),
                });
                continue;
            }

            let chars: Vec<char> = line.chars().collect();
            let mut start = 0usize;
            while start < chars.len() {
                let (end, text) = wrap_segment(&chars, start, wrap_w);
                out.push(VisualLine {
                    row,
                    start_col: start,
                    end_col: end,
                    text,
                });
                start = end;
            }
        }
        out
    }

    fn line_len_chars(&self, row: usize) -> usize {
        self.lines.get(row).map(|s| s.chars().count()).unwrap_or(0)
    }
}

fn wrap_segment(chars: &[char], start: usize, max_w: usize) -> (usize, String) {
    // Prefer breaking on whitespace, but fall back to a hard break.
    let mut w = 0usize;
    let mut end = start;
    let mut last_space: Option<usize> = None;

    while end < chars.len() {
        let cw = chars[end].width().unwrap_or(0).max(1);
        if w + cw > max_w {
            break;
        }
        if chars[end].is_whitespace() {
            last_space = Some(end);
        }
        w += cw;
        end += 1;
    }

    if end == start {
        end = (start + 1).min(chars.len());
    } else if end < chars.len() {
        if let Some(sp) = last_space {
            if sp > start {
                end = sp;
            }
        }
    }

    let mut text: String = chars[start..end].iter().collect();
    while text.ends_with(' ') || text.ends_with('\t') {
        text.pop();
    }
    (end, text)
}

fn slice_chars(s: &str, start: usize, end: usize) -> String {
    s.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn char_to_byte_index(s: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or_else(|| s.len())
}

fn col_at_x(s: &str, start_col: usize, end_col: usize, want_x: usize) -> usize {
    let mut x = 0usize;
    let mut col = start_col;
    for c in s
        .chars()
        .skip(start_col)
        .take(end_col.saturating_sub(start_col))
    {
        let cw = c.width().unwrap_or(0).max(1);
        if x + cw > want_x {
            break;
        }
        x += cw;
        col += 1;
    }
    col
}
