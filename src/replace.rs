//! User autocorrect rules: `replace = cube control => kubectl` in dictate.ini.
//!
//! Matching ignores case (and Russian ё/е), works on whole words only, and a space
//! in the pattern also matches a hyphen ("code review" catches "code-review").
//! If the matched text starts with a capital letter, so does the replacement.

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    words: Vec<Vec<char>>,
    to: String,
}

impl Rule {
    /// Parses `pattern => replacement`.
    pub fn parse(s: &str) -> Option<Rule> {
        let (from, to) = s.split_once("=>")?;
        let words: Vec<Vec<char>> = from
            .split(|c: char| c.is_whitespace() || is_dash(c))
            .filter(|w| !w.is_empty())
            .map(|w| w.chars().map(fold).collect())
            .collect();
        let to = to.trim();
        (!words.is_empty() && !to.is_empty()).then(|| Rule { words, to: to.to_string() })
    }
}

fn is_dash(c: char) -> bool {
    matches!(c, '-' | '‐' | '‑' | '–' | '—')
}

fn is_sep(c: char) -> bool {
    c.is_whitespace() || is_dash(c)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn fold(c: char) -> char {
    let l = c.to_lowercase().next().unwrap_or(c);
    if l == 'ё' { 'е' } else { l }
}

/// Length in chars of a match of `rule` starting at `i`, if any.
fn match_at(t: &[char], i: usize, rule: &Rule) -> Option<usize> {
    if i > 0 && is_word(t[i - 1]) {
        return None;
    }
    let mut j = i;
    for (k, w) in rule.words.iter().enumerate() {
        if k > 0 {
            let start = j;
            while j < t.len() && is_sep(t[j]) {
                j += 1;
            }
            if j == start {
                return None;
            }
        }
        for &pc in w {
            if j >= t.len() || fold(t[j]) != pc {
                return None;
            }
            j += 1;
        }
    }
    if j < t.len() && is_word(t[j]) {
        return None;
    }
    Some(j - i)
}

pub fn apply(text: &str, rules: &[Rule]) -> String {
    if rules.is_empty() {
        return text.to_string();
    }
    let t: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    'outer: while i < t.len() {
        for r in rules {
            if let Some(n) = match_at(&t, i, r) {
                let mut rep = r.to.chars();
                if t[i].is_uppercase()
                    && let Some(first) = rep.next()
                {
                    out.extend(first.to_uppercase());
                    out.extend(rep);
                } else {
                    out.push_str(&r.to);
                }
                i += n;
                continue 'outer;
            }
        }
        out.push(t[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(list: &[&str]) -> Vec<Rule> {
        list.iter().map(|s| Rule::parse(s).unwrap()).collect()
    }

    #[test]
    fn multi_word_variants() {
        let r = rules(&["cube control => kubectl", "cube ctl => kubectl"]);
        assert_eq!(apply("Run cube control get pods.", &r), "Run kubectl get pods.");
        assert_eq!(apply("Cube-control is down", &r), "Kubectl is down");
        assert_eq!(apply("cube   ctl", &r), "kubectl");
        assert_eq!(apply("cube controller", &r), "cube controller"); // whole words only
        assert_eq!(apply("supercube control", &r), "supercube control");
    }

    #[test]
    fn cyrillic_capitalization_and_yo() {
        let r = rules(&["код-ревью => code review", "ёлка => ель"]);
        assert_eq!(apply("Код-ревью готово", &r), "Code review готово");
        assert_eq!(apply("сделай код ревью", &r), "сделай code review");
        assert_eq!(apply("Елка", &r), "Ель");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(Rule::parse("no arrow").is_none());
        assert!(Rule::parse(" => empty").is_none());
        assert!(Rule::parse("empty => ").is_none());
    }
}
