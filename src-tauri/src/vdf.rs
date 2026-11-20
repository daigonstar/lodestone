//! Minimal parser for Valve's KeyValues text format (libraryfolders.vdf, appmanifest_*.acf).

#[derive(Debug, Clone)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(v) => v.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(v) => v,
            Vdf::Str(_) => &[],
        }
    }
}

enum Tok {
    S(String),
    Open,
    Close,
}

pub fn parse(src: &str) -> Vdf {
    let toks = tokenize(src);
    let mut i = 0;
    Vdf::Obj(parse_obj(&toks, &mut i))
}

fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            '"' => {
                let mut buf = String::new();
                while let Some(c) = it.next() {
                    match c {
                        '\\' => match it.next() {
                            Some('n') => buf.push('\n'),
                            Some('t') => buf.push('\t'),
                            Some(o) => buf.push(o),
                            None => {}
                        },
                        '"' => break,
                        _ => buf.push(c),
                    }
                }
                out.push(Tok::S(buf));
            }
            '/' if it.peek() == Some(&'/') => {
                for c in it.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {}
            _ => {
                let mut buf = String::from(c);
                while let Some(&n) = it.peek() {
                    if n.is_whitespace() || n == '{' || n == '}' || n == '"' {
                        break;
                    }
                    buf.push(n);
                    it.next();
                }
                out.push(Tok::S(buf));
            }
        }
    }
    out
}

fn parse_obj(t: &[Tok], i: &mut usize) -> Vec<(String, Vdf)> {
    let mut v = Vec::new();
    while *i < t.len() {
        match &t[*i] {
            Tok::Close => {
                *i += 1;
                break;
            }
            Tok::Open => *i += 1,
            Tok::S(k) => {
                let k = k.clone();
                *i += 1;
                match t.get(*i) {
                    Some(Tok::S(val)) => {
                        v.push((k, Vdf::Str(val.clone())));
                        *i += 1;
                    }
                    Some(Tok::Open) => {
                        *i += 1;
                        let o = parse_obj(t, i);
                        v.push((k, Vdf::Obj(o)));
                    }
                    _ => {}
                }
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested() {
        let v = parse(r#""AppState" { "appid" "10" "name" "Game \"X\"" "Sub" { "a" "1" } }"#);
        let app = v.get("appstate").unwrap();
        assert_eq!(app.str("appid"), Some("10"));
        assert_eq!(app.str("name"), Some("Game \"X\""));
        assert_eq!(app.get("sub").unwrap().str("a"), Some("1"));
    }
}
