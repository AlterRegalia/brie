//! A small Markdown renderer for the preview pane: headings, lists, checkboxes, quotes,
//! fenced code, tables, rules, and inline code, bold, italic and links.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme;

pub fn render(src: &str) -> Vec<Line<'static>> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out: Vec<Line<'static>> = vec![];
    let mut in_code = false;
    for (i, raw) in lines.iter().enumerate() {
        let Some(line) = view_line(raw, lines.get(i + 1).copied(), &mut in_code, false) else {
            continue;
        };
        if !out.is_empty()
            && !in_code
            && heading(raw.trim_start()).is_some_and(|(level, _)| level == 1)
        {
            out.push(Line::raw(""));
        }
        out.push(line);
    }
    out
}

/// One rendered line for each source line, for the editor: the lines in `raw` (the cursor line, or the
/// selection) show their source with the markers, every other line shows the rendered form.
pub fn live(lines: &[String], raw: std::ops::RangeInclusive<usize>) -> Vec<Line<'static>> {
    let mut in_code = false;
    let mut out = Vec::with_capacity(lines.len());
    for (i, text) in lines.iter().enumerate() {
        let before = in_code;
        let next = lines.get(i + 1).map(String::as_str);
        let rendered = view_line(text, next, &mut in_code, true);
        out.push(if raw.contains(&i) {
            source_line(text, &mut { before })
        } else {
            rendered.unwrap_or_default()
        });
    }
    out
}

/// The rendered form of one source line. `in_code` carries the fence state from line to line.
/// With `keep`, fence lines and table separators stay as thin rules, so each source line gives one line;
/// without it they give `None`.
fn view_line(
    raw: &str,
    next: Option<&str>,
    in_code: &mut bool,
    keep: bool,
) -> Option<Line<'static>> {
    let t = theme::get();
    let trimmed = raw.trim_start();

    if trimmed.starts_with("```") {
        let opening = !*in_code;
        *in_code = opening;
        let lang = trimmed.trim_start_matches('`').trim();
        return match (opening, keep) {
            (true, _) if !lang.is_empty() => Some(Line::styled(
                format!("  ╌╌ {lang}"),
                Style::new().fg(t.muted),
            )),
            (_, true) => Some(Line::styled("  ╌╌", Style::new().fg(t.muted))),
            _ => None,
        };
    }
    if *in_code {
        return Some(Line::styled(
            format!("  {raw}"),
            Style::new().fg(t.info).bg(t.bg),
        ));
    }
    if trimmed.is_empty() {
        return Some(Line::raw(""));
    }
    if let Some((level, text)) = heading(trimmed) {
        let (mark, style) = match level {
            1 => ("█ ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            2 => ("▌ ", Style::new().fg(t.text).add_modifier(Modifier::BOLD)),
            _ => ("› ", Style::new().fg(t.info).add_modifier(Modifier::BOLD)),
        };
        let text = if level == 1 {
            text.to_uppercase()
        } else {
            text.to_string()
        };
        let mut spans = vec![Span::styled(mark, Style::new().fg(t.accent))];
        spans.extend(inline(&text, style));
        return Some(Line::from(spans));
    }
    if is_rule(trimmed) {
        return Some(Line::styled("─".repeat(40), Style::new().fg(t.line)));
    }
    if let Some(rest) = trimmed.strip_prefix('>') {
        let mut spans = vec![Span::styled("┃ ", Style::new().fg(t.line))];
        spans.extend(inline(rest.trim_start(), Style::new().fg(t.muted)));
        return Some(Line::from(spans));
    }
    if trimmed.starts_with('|') {
        if is_table_separator(trimmed) {
            return keep.then(|| Line::styled("─".repeat(40), Style::new().fg(t.line)));
        }
        let header = next.is_some_and(is_table_separator);
        let style = if header {
            Style::new().fg(t.text).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.text)
        };
        let mut spans = vec![];
        for (n, cell) in trimmed.trim_matches('|').split('|').enumerate() {
            if n > 0 {
                spans.push(Span::styled(" │ ", Style::new().fg(t.line)));
            }
            spans.extend(inline(cell.trim(), style));
        }
        return Some(Line::from(spans));
    }
    if let Some((indent, body)) = bullet(raw) {
        let mut spans = vec![Span::raw(" ".repeat(indent))];
        let (mark, rest) = if let Some(r) = body.strip_prefix("[ ] ") {
            ("☐ ", r)
        } else if let Some(r) = body
            .strip_prefix("[x] ")
            .or_else(|| body.strip_prefix("[X] "))
        {
            ("☑ ", r)
        } else {
            ("• ", body)
        };
        spans.push(Span::styled(mark, Style::new().fg(t.accent)));
        spans.extend(inline(rest, Style::new().fg(t.text)));
        return Some(Line::from(spans));
    }
    if let Some((indent, num, body)) = numbered(raw) {
        let mut spans = vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(format!("{num}. "), Style::new().fg(t.accent)),
        ];
        spans.extend(inline(body, Style::new().fg(t.text)));
        return Some(Line::from(spans));
    }
    Some(Line::from(inline(trimmed, Style::new().fg(t.text))))
}

fn heading(s: &str) -> Option<(usize, &str)> {
    let level = s.chars().take_while(|&c| c == '#').count();
    let rest = s[level..].strip_prefix(' ')?;
    (1..=6).contains(&level).then_some((level, rest.trim()))
}

fn is_rule(s: &str) -> bool {
    let s: String = s.split_whitespace().collect();
    s.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|c| s.chars().all(|x| c.starts_with(x)))
}

fn is_table_separator(s: &str) -> bool {
    let s = s.trim();
    s.starts_with('|') && s.contains('-') && s.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn bullet(raw: &str) -> Option<(usize, &str)> {
    let trimmed = raw.trim_start();
    let body = ["- ", "* ", "+ "]
        .iter()
        .find_map(|p| trimmed.strip_prefix(p))?;
    Some((raw.len() - trimmed.len(), body))
}

fn numbered(raw: &str) -> Option<(usize, &str, &str)> {
    let trimmed = raw.trim_start();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let body = trimmed[digits..].strip_prefix(". ")?;
    Some((raw.len() - trimmed.len(), &trimmed[..digits], body))
}

fn find(chars: &[char], from: usize, pat: &[char]) -> Option<usize> {
    (from..chars.len().saturating_sub(pat.len() - 1)).find(|&k| chars[k..].starts_with(pat))
}

fn flush(spans: &mut Vec<Span<'static>>, buf: &mut String, style: Style) {
    if !buf.is_empty() {
        spans.push(Span::styled(std::mem::take(buf), style));
    }
}

fn inline(s: &str, base: Style) -> Vec<Span<'static>> {
    let t = theme::get();
    let chars: Vec<char> = s.chars().collect();
    let mut spans = vec![];
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`'
            && let Some(end) = find(&chars, i + 1, &['`'])
        {
            flush(&mut spans, &mut buf, base);
            let code: String = chars[i + 1..end].iter().collect();
            spans.push(Span::styled(code, Style::new().fg(t.info).bg(t.line)));
            i = end + 1;
        } else if c == '*'
            && chars.get(i + 1) == Some(&'*')
            && let Some(end) = find(&chars, i + 2, &['*', '*'])
        {
            flush(&mut spans, &mut buf, base);
            let inner: String = chars[i + 2..end].iter().collect();
            spans.extend(inline(&inner, base.add_modifier(Modifier::BOLD)));
            i = end + 2;
        } else if c == '*'
            && chars.get(i + 1).is_some_and(|n| !n.is_whitespace())
            && let Some(end) = find(&chars, i + 1, &['*'])
        {
            flush(&mut spans, &mut buf, base);
            let inner: String = chars[i + 1..end].iter().collect();
            spans.extend(inline(&inner, base.add_modifier(Modifier::ITALIC)));
            i = end + 1;
        } else if c == '['
            && let Some(mid) = find(&chars, i + 1, &[']', '('])
            && let Some(end) = find(&chars, mid + 2, &[')'])
        {
            flush(&mut spans, &mut buf, base);
            let label: String = chars[i + 1..mid].iter().collect();
            spans.push(Span::styled(
                label,
                Style::new().fg(t.info).add_modifier(Modifier::UNDERLINED),
            ));
            i = end + 1;
        } else {
            buf.push(c);
            i += 1;
        }
    }
    flush(&mut spans, &mut buf, base);
    spans
}

/// One source line with its markers visible and styled. `in_code` carries the fence state.
fn source_line(raw: &str, in_code: &mut bool) -> Line<'static> {
    let t = theme::get();
    let trimmed = raw.trim_start();
    if trimmed.starts_with("```") {
        *in_code = !*in_code;
        return Line::styled(raw.to_string(), Style::new().fg(t.muted));
    }
    if *in_code {
        return Line::styled(raw.to_string(), Style::new().fg(t.info));
    }
    if let Some((level, _)) = heading(trimmed) {
        let style = match level {
            1 => Style::new().fg(t.accent),
            2 => Style::new().fg(t.text),
            _ => Style::new().fg(t.info),
        };
        return Line::styled(raw.to_string(), style.add_modifier(Modifier::BOLD));
    }
    if is_rule(trimmed) {
        return Line::styled(raw.to_string(), Style::new().fg(t.line));
    }
    if trimmed.starts_with('>') {
        return Line::styled(raw.to_string(), Style::new().fg(t.muted));
    }
    if let Some((indent, body)) = bullet(raw) {
        let marker = &trimmed[..trimmed.len() - body.len()];
        let mut spans = vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(marker.to_string(), Style::new().fg(t.accent)),
        ];
        spans.extend(source_inline(body));
        return Line::from(spans);
    }
    if let Some((indent, num, body)) = numbered(raw) {
        let mut spans = vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(format!("{num}. "), Style::new().fg(t.accent)),
        ];
        spans.extend(source_inline(body));
        return Line::from(spans);
    }
    Line::from(source_inline(raw))
}

/// Inline styling that keeps every character: code, bold and links show their markers.
fn source_inline(s: &str) -> Vec<Span<'static>> {
    let t = theme::get();
    let base = Style::new().fg(t.text);
    let chars: Vec<char> = s.chars().collect();
    let mut spans = vec![];
    let mut buf = String::new();
    let mut i = 0;
    let take = |from: usize, to: usize| chars[from..to].iter().collect::<String>();
    while i < chars.len() {
        let end = if chars[i] == '`' {
            find(&chars, i + 1, &['`']).map(|e| (e + 1, Style::new().fg(t.info)))
        } else if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            find(&chars, i + 2, &['*', '*']).map(|e| (e + 2, base.add_modifier(Modifier::BOLD)))
        } else if chars[i] == '[' {
            find(&chars, i + 1, &[']', '(']).and_then(|m| {
                find(&chars, m + 2, &[')']).map(|e| {
                    (
                        e + 1,
                        Style::new().fg(t.info).add_modifier(Modifier::UNDERLINED),
                    )
                })
            })
        } else {
            None
        };
        match end {
            Some((to, style)) => {
                flush(&mut spans, &mut buf, base);
                spans.push(Span::styled(take(i, to), style));
                i = to;
            }
            None => {
                buf.push(chars[i]);
                i += 1;
            }
        }
    }
    flush(&mut spans, &mut buf, base);
    spans
}
