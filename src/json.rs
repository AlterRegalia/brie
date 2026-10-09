//! A small JSON reader and writer, enough for the MCP messages. No dependency.

#[derive(Clone, Debug)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn num(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn write(&self) -> String {
        match self {
            Json::Null => "null".into(),
            Json::Bool(b) => b.to_string(),
            Json::Num(n) => n.to_string(),
            Json::Str(s) => quote(s),
            Json::Arr(a) => format!(
                "[{}]",
                a.iter().map(Json::write).collect::<Vec<_>>().join(",")
            ),
            Json::Obj(o) => format!(
                "{{{}}}",
                o.iter()
                    .map(|(k, v)| format!("{}:{}", quote(k), v.write()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn parse(text: &str) -> Option<Json> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let value = value(&chars, &mut at)?;
    skip(&chars, &mut at);
    (at == chars.len()).then_some(value)
}

fn skip(c: &[char], at: &mut usize) {
    while c.get(*at).is_some_and(|x| x.is_whitespace()) {
        *at += 1;
    }
}

fn value(c: &[char], at: &mut usize) -> Option<Json> {
    skip(c, at);
    match c.get(*at)? {
        '{' => {
            *at += 1;
            let mut items = vec![];
            skip(c, at);
            if c.get(*at) == Some(&'}') {
                *at += 1;
                return Some(Json::Obj(items));
            }
            loop {
                skip(c, at);
                let key = string(c, at)?;
                skip(c, at);
                if c.get(*at) != Some(&':') {
                    return None;
                }
                *at += 1;
                items.push((key, value(c, at)?));
                skip(c, at);
                match c.get(*at)? {
                    ',' => *at += 1,
                    '}' => {
                        *at += 1;
                        return Some(Json::Obj(items));
                    }
                    _ => return None,
                }
            }
        }
        '[' => {
            *at += 1;
            let mut items = vec![];
            skip(c, at);
            if c.get(*at) == Some(&']') {
                *at += 1;
                return Some(Json::Arr(items));
            }
            loop {
                items.push(value(c, at)?);
                skip(c, at);
                match c.get(*at)? {
                    ',' => *at += 1,
                    ']' => {
                        *at += 1;
                        return Some(Json::Arr(items));
                    }
                    _ => return None,
                }
            }
        }
        '"' => string(c, at).map(Json::Str),
        't' => word(c, at, "true", Json::Bool(true)),
        'f' => word(c, at, "false", Json::Bool(false)),
        'n' => word(c, at, "null", Json::Null),
        _ => {
            let start = *at;
            while c
                .get(*at)
                .is_some_and(|x| matches!(x, '-' | '+' | '.' | 'e' | 'E' | '0'..='9'))
            {
                *at += 1;
            }
            c[start..*at]
                .iter()
                .collect::<String>()
                .parse()
                .ok()
                .map(Json::Num)
        }
    }
}

fn word(c: &[char], at: &mut usize, w: &str, v: Json) -> Option<Json> {
    let end = *at + w.len();
    (c.get(*at..end)?.iter().collect::<String>() == w).then(|| {
        *at = end;
        v
    })
}

fn string(c: &[char], at: &mut usize) -> Option<String> {
    if c.get(*at) != Some(&'"') {
        return None;
    }
    *at += 1;
    let mut out = String::new();
    loop {
        let ch = *c.get(*at)?;
        *at += 1;
        match ch {
            '"' => return Some(out),
            '\\' => {
                let esc = *c.get(*at)?;
                *at += 1;
                match esc {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let mut code = hex4(c, at)?;
                        if (0xD800..0xDC00).contains(&code)
                            && c.get(*at) == Some(&'\\')
                            && c.get(*at + 1) == Some(&'u')
                        {
                            *at += 2;
                            let low = hex4(c, at)?;
                            code = 0x10000
                                + ((code - 0xD800) << 10)
                                + (low.wrapping_sub(0xDC00) & 0x3FF);
                        }
                        out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    other => out.push(other),
                }
            }
            other => out.push(other),
        }
    }
}

fn hex4(c: &[char], at: &mut usize) -> Option<u32> {
    let s: String = c.get(*at..*at + 4)?.iter().collect();
    *at += 4;
    u32::from_str_radix(&s, 16).ok()
}
