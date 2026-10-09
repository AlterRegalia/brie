//! The built-in static file server: Markdown pages, SPA fallback, byte ranges. 127.0.0.1 only.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

pub fn start(dir: PathBuf) -> std::io::Result<u16> {
    let mut last = None;
    for port in 4100..4200 {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                thread::spawn(move || {
                    for stream in listener.incoming().flatten() {
                        let dir = dir.clone();
                        thread::spawn(move || {
                            let _ = handle(stream, &dir);
                        });
                    }
                });
                return Ok(port);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free port")))
}

fn handle(mut s: TcpStream, root: &Path) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    let n = s.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let mut parts = req.lines().next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        return respond(&mut s, 405, "text/plain", b"method not allowed", true, None);
    }
    let head = method == "HEAD";
    let range = req.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("range:")
            .map(|v| v.trim().to_string())
    });
    let (url_path, query) = {
        let no_hash = target.split('#').next().unwrap_or("/");
        no_hash.split_once('?').unwrap_or((no_hash, ""))
    };
    let rel = decode(url_path);
    if rel.split('/').any(|c| c == "..") {
        return respond(&mut s, 403, "text/plain", b"forbidden", head, None);
    }
    let mut full = root.join(rel.trim_start_matches('/'));
    if full.is_dir() {
        let index = full.join("index.html");
        if index.is_file() {
            full = index;
        } else {
            let page = listing(&full, url_path);
            return respond(
                &mut s,
                200,
                "text/html; charset=utf-8",
                page.as_bytes(),
                head,
                None,
            );
        }
    }
    if !full.is_file() {
        // A path with no extension is a client-side route: give it the app, as a SPA host does.
        let spa = root.join("index.html");
        if Path::new(&rel).extension().is_none() && spa.is_file() {
            full = spa;
        }
    }
    let Ok(body) = fs::read(&full) else {
        return respond(&mut s, 404, "text/plain", b"not found", head, None);
    };
    let is_md = full
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"));
    if is_md && !query.split('&').any(|q| q == "raw") {
        let page = md_page(&full, &String::from_utf8_lossy(&body));
        return respond(
            &mut s,
            200,
            "text/html; charset=utf-8",
            page.as_bytes(),
            head,
            None,
        );
    }
    let total = body.len();
    if let Some(r) = range.as_deref().and_then(|r| r.strip_prefix("bytes=")) {
        return match parse_range(r, total) {
            Some((from, to)) => respond(
                &mut s,
                206,
                mime(&full),
                &body[from..=to],
                head,
                Some((from, to, total)),
            ),
            None => respond(
                &mut s,
                416,
                "text/plain",
                b"range not satisfiable",
                head,
                Some((0, 0, total)),
            ),
        };
    }
    respond(&mut s, 200, mime(&full), &body, head, None)
}

/// One byte range: `a-b`, `a-` or `-n`. Returns inclusive bounds inside `total`.
fn parse_range(r: &str, total: usize) -> Option<(usize, usize)> {
    let first = r.split(',').next()?;
    let (a, b) = first.split_once('-')?;
    if total == 0 {
        return None;
    }
    let (from, to) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: usize = n.parse().ok()?;
            (total.saturating_sub(n), total - 1)
        }
        (a, "") => (a.parse().ok()?, total - 1),
        (a, b) => (a.parse().ok()?, b.parse::<usize>().ok()?.min(total - 1)),
    };
    (from <= to && from < total).then_some((from, to))
}

fn respond(
    s: &mut TcpStream,
    code: u16,
    mime: &str,
    body: &[u8],
    head: bool,
    range: Option<(usize, usize, usize)>,
) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        206 => "Partial Content",
        403 => "Forbidden",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "Method Not Allowed",
    };
    let content_range = match range {
        Some((from, to, total)) if code == 206 => {
            format!("Content-Range: bytes {from}-{to}/{total}\r\n")
        }
        Some((_, _, total)) => format!("Content-Range: bytes */{total}\r\n"),
        None => String::new(),
    };
    write!(
        s,
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n{content_range}Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if !head {
        s.write_all(body)?;
    }
    Ok(())
}

// ---- Markdown as a page ----------------------------------------------------------------------

fn md_page(path: &Path, src: &str) -> String {
    let title = escape(&path.file_name().unwrap_or_default().to_string_lossy());
    format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
<title>{title}</title><style>\
:root{{color-scheme:dark}}body{{background:#07080c;color:#f4f5f8;font:16px/1.6 system-ui,sans-serif;margin:0}}\
main{{max-width:760px;margin:0 auto;padding:24px 16px 64px}}\
h1{{color:#ffc400;text-transform:uppercase;letter-spacing:.02em}}h2,h3{{margin-top:1.6em}}\
a{{color:#7cc4ff}}code{{background:#16171c;padding:1px 5px;font-family:ui-monospace,monospace;font-size:.92em}}\
pre{{background:#0e1016;border:1px solid #2a2c34;padding:12px;overflow:auto}}pre code{{background:none;padding:0}}\
blockquote{{border-left:3px solid #2a2c34;margin:0;padding-left:14px;color:#a9b0be}}\
table{{border-collapse:collapse}}td,th{{border:1px solid #2a2c34;padding:4px 10px}}hr{{border:0;border-top:1px solid #2a2c34}}\
img{{max-width:100%}}</style><main>{}</main>",
        md_html(src)
    )
}

fn md_html(src: &str) -> String {
    let mut out = String::new();
    let mut para: Vec<String> = vec![];
    let mut list: Option<&str> = None;
    let mut in_code = false;
    let mut in_table = false;
    let lines: Vec<&str> = src.lines().collect();
    let close_para = |out: &mut String, para: &mut Vec<String>| {
        if !para.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", inline_html(&para.join(" "))));
            para.clear();
        }
    };
    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim_start();
        if t.starts_with("```") {
            close_para(&mut out, &mut para);
            out.push_str(if in_code {
                "</code></pre>\n"
            } else {
                "<pre><code>"
            });
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push_str(&escape(raw));
            out.push('\n');
            continue;
        }
        let is_item = t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ");
        let num = t.chars().take_while(char::is_ascii_digit).count();
        let is_num = num > 0 && t[num..].starts_with(". ");
        if list.is_some() && !(is_item || is_num) {
            out.push_str(&format!("</{}>\n", list.take().unwrap_or("ul")));
        }
        if in_table && !t.starts_with('|') {
            out.push_str("</table>\n");
            in_table = false;
        }
        if t.is_empty() {
            close_para(&mut out, &mut para);
        } else if let Some(level) = (1..=6).find(|&n| t.starts_with(&format!("{} ", "#".repeat(n))))
        {
            close_para(&mut out, &mut para);
            out.push_str(&format!(
                "<h{level}>{}</h{level}>\n",
                inline_html(t[level + 1..].trim())
            ));
        } else if t.chars().filter(|c| !c.is_whitespace()).count() >= 3
            && (t.chars().all(|c| matches!(c, '-' | ' '))
                || t.chars().all(|c| matches!(c, '*' | ' ')))
        {
            close_para(&mut out, &mut para);
            out.push_str("<hr>\n");
        } else if let Some(rest) = t.strip_prefix('>') {
            close_para(&mut out, &mut para);
            out.push_str(&format!(
                "<blockquote>{}</blockquote>\n",
                inline_html(rest.trim())
            ));
        } else if is_item || is_num {
            close_para(&mut out, &mut para);
            let kind = if is_num { "ol" } else { "ul" };
            if list != Some(kind) {
                if let Some(old) = list {
                    out.push_str(&format!("</{old}>\n"));
                }
                out.push_str(&format!("<{kind}>\n"));
                list = Some(kind);
            }
            let body = if is_num { &t[num + 2..] } else { &t[2..] };
            let body = body
                .strip_prefix("[ ] ")
                .map(|r| format!("\u{2610} {r}"))
                .or_else(|| body.strip_prefix("[x] ").map(|r| format!("\u{2611} {r}")))
                .unwrap_or_else(|| body.to_string());
            out.push_str(&format!("<li>{}</li>\n", inline_html(&body)));
        } else if t.starts_with('|') {
            close_para(&mut out, &mut para);
            let sep = t.trim().chars().all(|c| matches!(c, '|' | '-' | ':' | ' '));
            if sep {
                continue;
            }
            let header = !in_table
                && lines.get(i + 1).is_some_and(|n| {
                    let n = n.trim();
                    n.starts_with('|')
                        && n.contains('-')
                        && n.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
                });
            if !in_table {
                out.push_str("<table>\n");
                in_table = true;
            }
            let tag = if header { "th" } else { "td" };
            let cells: String = t
                .trim()
                .trim_matches('|')
                .split('|')
                .map(|c| format!("<{tag}>{}</{tag}>", inline_html(c.trim())))
                .collect();
            out.push_str(&format!("<tr>{cells}</tr>\n"));
        } else {
            para.push(t.to_string());
        }
    }
    close_para(&mut out, &mut para);
    if let Some(kind) = list {
        out.push_str(&format!("</{kind}>\n"));
    }
    if in_table {
        out.push_str("</table>\n");
    }
    if in_code {
        out.push_str("</code></pre>\n");
    }
    out
}

/// Inline Markdown on escaped text: code, bold, italic and links (http, https, or relative only).
fn inline_html(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let find = |from: usize, pat: &[char]| -> Option<usize> {
        (from..chars.len().saturating_sub(pat.len() - 1)).find(|&k| chars[k..].starts_with(pat))
    };
    let text = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`'
            && let Some(e) = find(i + 1, &['`'])
        {
            out.push_str(&format!("<code>{}</code>", escape(&text(i + 1, e))));
            i = e + 1;
        } else if chars[i] == '*'
            && chars.get(i + 1) == Some(&'*')
            && let Some(e) = find(i + 2, &['*', '*'])
        {
            out.push_str(&format!(
                "<strong>{}</strong>",
                inline_html(&text(i + 2, e))
            ));
            i = e + 2;
        } else if chars[i] == '*'
            && chars.get(i + 1).is_some_and(|c| !c.is_whitespace())
            && let Some(e) = find(i + 1, &['*'])
        {
            out.push_str(&format!("<em>{}</em>", inline_html(&text(i + 1, e))));
            i = e + 1;
        } else if chars[i] == '['
            && let Some(m) = find(i + 1, &[']', '('])
            && let Some(e) = find(m + 2, &[')'])
        {
            let url = text(m + 2, e);
            let safe =
                !url.contains(':') || url.starts_with("http://") || url.starts_with("https://");
            let label = inline_html(&text(i + 1, m));
            if safe {
                out.push_str(&format!(
                    "<a href=\"{}\">{label}</a>",
                    escape(&url).replace('"', "&quot;")
                ));
            } else {
                out.push_str(&label);
            }
            i = e + 1;
        } else {
            out.push_str(&escape(&chars[i].to_string()));
            i += 1;
        }
    }
    out
}

fn listing(dir: &Path, url_path: &str) -> String {
    let base = if url_path.ends_with('/') {
        url_path.to_string()
    } else {
        format!("{url_path}/")
    };
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let rows: String = names
        .iter()
        .map(|n| {
            format!(
                "<li><a href=\"{base}{}\">{}</a></li>",
                n.replace(' ', "%20"),
                escape(n)
            )
        })
        .collect();
    format!(
        "<!doctype html><meta charset=utf-8><title>{}</title><ul>{rows}</ul>",
        escape(&base)
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = hex(b[i + 1]).zip(hex(b[i + 2])).map(|(h, l)| h * 16 + l)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

fn mime(p: &Path) -> &'static str {
    match p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "xml" => "application/xml",
        "csv" => "text/csv; charset=utf-8",
        "md" | "txt" | "log" | "yaml" | "yml" | "toml" | "ts" | "tsx" | "rs" | "py" | "sh" => {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    }
}
