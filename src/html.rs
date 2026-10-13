//! The Markdown renderer's output as HTML, for the daemon's brief window.
//!
//! `md` returns ratatui `Line`s of `Span`s, each carrying a `Style`: a foreground, a background and
//! three modifiers. That is the whole vocabulary, so this is a color lookup and a handful of CSS
//! rules — the window shows exactly the cells the terminal shows, which is the point: one renderer,
//! so the TUI and the window cannot drift.

use std::collections::HashMap;
use std::fmt::Write as _;

use ratatui::style::{Color, Modifier, Style};

use crate::md;
use crate::theme;

fn hex(c: Option<Color>) -> Option<String> {
    match c? {
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        _ => None,
    }
}

/// The CSS for one cell style: a color, and the modifiers the renderer can set.
fn rules(style: &Style) -> String {
    let mut css = String::new();
    if let Some(fg) = hex(style.fg) {
        let _ = write!(css, "color:{fg};");
    }
    if let Some(bg) = hex(style.bg) {
        let _ = write!(css, "background:{bg};");
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        css.push_str("font-weight:700;");
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        css.push_str("font-style:italic;");
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        css.push_str("text-decoration:underline;");
    }
    css
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The rendered file: one `<div>` per line, one `<span>` per run of cells that share a style. Styles
/// are pooled, so a whole brief needs about a dozen rules rather than one per span.
pub fn body(src: &str) -> String {
    let mut classes: HashMap<String, String> = HashMap::new();
    let mut pooled: Vec<String> = vec![];
    let mut class = |css: String| -> String {
        if css.is_empty() {
            return String::new();
        }
        classes
            .entry(css.clone())
            .or_insert_with(|| {
                pooled.push(css);
                format!("s{}", pooled.len())
            })
            .clone()
    };

    let mut html = String::new();
    for line in md::render(src) {
        html.push_str("<div class=\"l\">");
        for span in &line.spans {
            if span.content.is_empty() {
                continue;
            }
            let name = class(rules(&span.style));
            let text = escape(&span.content);
            if name.is_empty() {
                html.push_str(&text);
            } else {
                let _ = write!(html, "<span class=\"{name}\">{text}</span>");
            }
        }
        html.push_str("</div>\n");
    }

    let mut css = String::new();
    for (ix, rules) in pooled.iter().enumerate() {
        let _ = writeln!(css, ".s{}{{{rules}}}", ix + 1);
    }
    css.push_str(&html);
    css
}

/// A whole page for the brief window: the file, in the TUI's own look, with the path across the top.
pub fn page(title: &str, path: &str, src: &str, close_token: &str) -> String {
    let t = theme::rgb();
    // The page is generated here, so every color in it comes from the theme, not from a stylesheet.
    let body = body(src);
    // The styles sit in the document rather than in an escaped string, so split them out again.
    let (styles, body) = split(&body);
    let c = |color: Color| match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#000000".to_string(),
    };
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>{title}</title><style>\n\
:root{{--bg:{bg};--surface:{surface};--line:{line};--ink:{text};--muted:{muted};--accent:{accent}}}\n\
*{{box-sizing:border-box}}\n\
html,body{{margin:0;height:100%}}\n\
body{{background:var(--bg);color:var(--ink);font:12.5px/1.35 ui-monospace,\"SF Mono\",Menlo,monospace;\
-webkit-font-smoothing:antialiased}}\n\
header{{display:flex;gap:10px;align-items:baseline;padding:9px 14px;background:var(--surface);\
border-bottom:1px solid var(--line);font-size:11.5px;color:var(--muted)}}\n\
header b{{color:var(--accent);font-weight:400;white-space:nowrap}}\n\
header .where{{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}}\n\
header .sp{{margin-left:auto;white-space:nowrap}}\n\
main{{padding:14px 16px 40px}}\n\
.l{{white-space:pre-wrap;word-break:break-word;min-height:1.35em}}\n\
{styles}\n\
</style></head>\n\
<body>\n\
<header><b>{name}</b><span class=\"where\">{path}</span><span class=\"sp\">esc closes</span></header>\n\
<main>{body}</main>\n\
<script>document.addEventListener(\"keydown\",function(e){{\
if(e.key===\"Escape\"){{try{{window.ipc.postMessage(\"view-close\\t{token}\")}}catch(e){{}}}}}});</script>\n\
</body></html>\n",
        name = escape(title),
        path = escape(path),
        styles = styles,
        body = body,
        bg = c(t.bg),
        surface = c(t.surface),
        line = c(t.line),
        text = c(t.text),
        muted = c(t.muted),
        accent = c(t.accent),
        token = escape(close_token),
    )
}

/// The pooled `.s…` rules at the front of `body`'s output, and the rest.
fn split(pooled_and_body: &str) -> (String, String) {
    let mut at = 0;
    for line in pooled_and_body.split_inclusive('\n') {
        if !line.starts_with(".s") {
            break;
        }
        at += line.len();
    }
    (
        pooled_and_body[..at].trim_end().to_string(),
        pooled_and_body[at..].to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_of(src: &str) -> String {
        body(src)
    }

    /// Just the words, so an assertion is about the text and not about the markup between it.
    fn text(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for c in html.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn the_renderers_own_rules_survive_the_bridge() {
        let out = body_of("# Plan — the podium\n\nStatus: done\n\n- one *said*\n\n> quoted\n");
        assert!(
            out.contains("PLAN — THE PODIUM"),
            "the H1 is uppercased by md.rs: {out}"
        );
        assert!(
            out.contains("color:#ffc400"),
            "the H1 keeps the accent: {out}"
        );
        assert!(out.contains("font-style:italic"), "italic survives: {out}");
        assert!(
            out.contains("&lt;") || !out.contains("&"),
            "no markers leak: {out}"
        );
        assert!(!out.contains("## "), "the Markdown markers are gone: {out}");
    }

    #[test]
    fn a_table_stays_pipes_because_a_terminal_line_is_a_row_of_cells() {
        let out = body_of("| Step | Brief |\n| ---- | ----- |\n| 1 | px1 |\n");
        let words = text(&out);
        assert!(words.contains("Step │ Brief"), "{words}");
        assert!(
            words.contains("1 │ px1"),
            "the body follows the header: {words}"
        );
        assert!(
            !words.contains('|'),
            "the pipes became cell separators: {words}"
        );
    }

    #[test]
    fn the_window_shows_the_file_not_its_html() {
        let out = body_of("a <script>alert(1)</script> & an ampersand\n");
        assert!(out.contains("&lt;script&gt;"), "{out}");
        assert!(!out.contains("<script>alert"), "{out}");
        assert!(out.contains("&amp;"), "{out}");
    }

    #[test]
    fn styles_are_pooled_so_the_bridge_stays_a_handful_of_rules() {
        let src = "# Top\n\n## Two\n\n- a\n- b\n\n`code` and **bold** and *it* and > quote\n";
        let out = body_of(src);
        let rules: Vec<&str> = out.lines().filter(|l| l.starts_with(".s")).collect();
        assert!(
            (6..=16).contains(&rules.len()),
            "{} rules for a whole file: {rules:?}",
            rules.len()
        );
        // every span refers to a rule that exists
        for name in out
            .match_indices("class=\"s")
            .map(|(i, _)| &out[i + 7..])
            .map(|r| {
                let end = r.find('"').unwrap_or(r.len());
                &r[..end]
            })
        {
            assert!(
                rules.iter().any(|l| l.starts_with(&format!(".{name}{{"))),
                "class {name} has no rule"
            );
        }
    }

    #[test]
    fn the_page_carries_the_theme_the_path_and_a_way_out() {
        let page = page(
            "09_10_2026_10:37_x.md",
            "/store/p/briefs/09_10_2026_10:37_x.md",
            "# Hi\n",
            "3",
        );
        assert!(page.starts_with("<!doctype html>"), "a whole document");
        assert!(page.contains("--accent:#ffc400"), "the theme's amber");
        assert!(page.contains("--bg:#07080c"));
        assert!(page.contains("09_10_2026_10:37_x.md"));
        assert!(
            page.contains("view-close\\t3"),
            "escape asks this window to close"
        );
        assert!(page.contains("HI"), "and the file is rendered in it");
    }
}
