//! WER / CER for the benchmark. Case, punctuation and ё/е are ignored;
//! Latin vs Cyrillic is NOT unified on purpose ("Docker" != "докер").

pub fn normalize(s: &str) -> String {
    let s: String = s
        .to_lowercase()
        .replace('ё', "е")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + (x != y) as usize).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// (word errors, reference words, char errors, reference chars)
pub fn errors(reference: &str, hypothesis: &str) -> (usize, usize, usize, usize) {
    let (r, h) = (normalize(reference), normalize(hypothesis));
    let rw: Vec<&str> = r.split(' ').filter(|w| !w.is_empty()).collect();
    let hw: Vec<&str> = h.split(' ').filter(|w| !w.is_empty()).collect();
    let rc: Vec<char> = r.chars().collect();
    let hc: Vec<char> = h.chars().collect();
    (edit_distance(&rw, &hw), rw.len(), edit_distance(&rc, &hc), rc.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wer_basics() {
        assert_eq!(errors("Привет, мир!", "привет мир"), (0, 2, 0, 10));
        assert_eq!(errors("запусти Docker", "запусти докер").0, 1);
        assert_eq!(errors("ёлка", "елка").0, 0);
    }
}
