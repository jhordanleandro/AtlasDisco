//! Padrões de nome: lista separada por ';' de curingas (`*.mp4;temp*`) ou expressões regulares.

use regex::{Regex, RegexBuilder};

#[derive(Clone, Debug)]
pub struct Matcher {
    rx: Vec<Regex>,
}

impl Matcher {
    /// `None` se a lista estiver vazia.
    pub fn new(list: &str, regex: bool) -> Result<Option<Matcher>, regex::Error> {
        let mut rx = Vec::new();
        for item in list
            .split([';', '|'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let src = if regex {
                item.to_string()
            } else {
                wildcard(item)
            };
            rx.push(RegexBuilder::new(&src).case_insensitive(true).build()?);
        }
        Ok(if rx.is_empty() {
            None
        } else {
            Some(Matcher { rx })
        })
    }

    pub fn is_match(&self, s: &str) -> bool {
        self.rx.iter().any(|r| r.is_match(s))
    }
}

/// Curinga → regex ancorada. `*` = qualquer sequência, `?` = um caractere.
pub fn wildcard(p: &str) -> String {
    let mut s = String::from("^");
    for c in p.chars() {
        match c {
            '*' => s.push_str(".*"),
            '?' => s.push('.'),
            c => s.push_str(&regex::escape(&c.to_string())),
        }
    }
    s.push('$');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn curingas() {
        let m = Matcher::new("*.mp4; temp*", false).unwrap().unwrap();
        assert!(m.is_match("Filme.MP4"));
        assert!(m.is_match("tempfile"));
        assert!(!m.is_match("a.mp45"));
        assert!(Matcher::new(" ; ", false).unwrap().is_none());
    }
}
