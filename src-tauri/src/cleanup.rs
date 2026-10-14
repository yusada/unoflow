//! Deterministic transcript cleanup.
//!
//! OpenWhispr runs an AI pass to strip filler and tidy phrasing. We do a
//! conservative, rule-based version locally: it removes hesitation sounds and
//! immediate word repetitions, then tidies spacing and capitalisation.
//!
//! Deliberately conservative. Aggressive filler removal mangles meaning
//! ("like" is often a real word), so only unambiguous fillers are dropped.

/// Hesitation sounds that never carry meaning on their own.
const FILLERS: &[&str] = &["um", "uh", "erm", "er", "hmm", "mmm", "ah", "eh"];

/// Clean a raw transcript. `level` 0 = off, 1 = fillers + tidy, 2 = also
/// collapse immediate repetitions.
pub fn clean(text: &str, level: u8) -> String {
    if level == 0 || text.trim().is_empty() {
        return text.trim().to_string();
    }

    let mut out: Vec<String> = Vec::new();
    for raw in text.split_whitespace() {
        // Separate a trailing punctuation run so "um," and "um." are caught.
        let (word, tail) = split_trailing_punct(raw);
        let lower = word.to_lowercase();

        if FILLERS.contains(&lower.as_str()) {
            // Drop the filler, but keep a sentence-ending punctuation mark so
            // "I think, um. Next" doesn't lose the full stop.
            if tail.contains('.') || tail.contains('?') || tail.contains('!') {
                if let Some(last) = out.last_mut() {
                    if !last.ends_with(['.', '?', '!']) {
                        last.push('.');
                    }
                }
            }
            continue;
        }

        // Collapse an immediate repetition: "the the" -> "the".
        if level >= 2 {
            if let Some(last) = out.last() {
                let (lw, lt) = split_trailing_punct(last);
                if lw.to_lowercase() == lower && lt == tail {
                    continue;
                }
            }
        }

        out.push(raw.to_string());
    }

    let joined = out.join(" ");
    tidy(&joined)
}

/// Split `"word,"` into `("word", ",")`.
fn split_trailing_punct(s: &str) -> (&str, &str) {
    let end = s
        .char_indices()
        .rev()
        .find(|(_, c)| !matches!(c, ',' | '.' | '!' | '?' | ';' | ':' | '"' | '\''))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    (&s[..end], &s[end..])
}

/// Fix spacing before punctuation and capitalise sentence starts.
fn tidy(s: &str) -> String {
    let mut t = s.to_string();

    // No space before , . ! ? ; :
    for p in [',', '.', '!', '?', ';', ':'] {
        t = t.replace(&format!(" {p}"), &p.to_string());
    }
    // Collapse runs of spaces.
    while t.contains("  ") {
        t = t.replace("  ", " ");
    }
    t = t.trim().to_string();

    // Capitalise the first letter and the letter after . ! ?
    let mut chars: Vec<char> = t.chars().collect();
    let mut cap_next = true;
    for c in chars.iter_mut() {
        if cap_next && c.is_alphabetic() {
            *c = c.to_uppercase().next().unwrap_or(*c);
            cap_next = false;
        } else if matches!(c, '.' | '!' | '?') {
            cap_next = true;
        }
    }
    chars.into_iter().collect()
}

/// Expand a snippet when its trigger appears in `text` as a standalone phrase.
/// Returns the text with expansions applied.
pub fn expand_snippets(text: &str, snippets: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (trigger, expansion) in snippets {
        let trigger = trigger.trim();
        if trigger.is_empty() {
            continue;
        }
        out = replace_phrase_ci(&out, trigger, expansion);
    }
    out
}

/// Case-insensitive, word-boundary-aware phrase replacement.
fn replace_phrase_ci(haystack: &str, needle: &str, replacement: &str) -> String {
    let h: Vec<char> = haystack.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() || h.len() < n.len() {
        return haystack.to_string();
    }

    let mut out = String::new();
    let mut i = 0;
    while i < h.len() {
        let fits = i + n.len() <= h.len()
            && (0..n.len())
                .all(|k| h[i + k].to_lowercase().eq(n[k].to_lowercase()));
        // Require boundaries so "intro email" doesn't fire inside "intro emailx".
        let left_ok = i == 0 || !h[i - 1].is_alphanumeric();
        let right_ok = i + n.len() >= h.len() || !h[i + n.len()].is_alphanumeric();

        if fits && left_ok && right_ok {
            out.push_str(replacement);
            i += n.len();
        } else {
            out.push(h[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_fillers_and_tidies() {
        assert_eq!(clean("um so I think uh we should go", 1), "So I think we should go");
    }

    #[test]
    fn level_zero_is_passthrough() {
        let s = "  um hello  ";
        assert_eq!(clean(s, 0), "um hello");
    }

    #[test]
    fn collapses_repetition_at_level_two() {
        assert_eq!(clean("the the cat", 2), "The cat");
        // Level 1 leaves repetition alone.
        assert_eq!(clean("the the cat", 1), "The the cat");
    }

    #[test]
    fn fixes_space_before_punctuation() {
        assert_eq!(clean("hello , world .", 1), "Hello, world.");
    }

    #[test]
    fn expands_snippet_by_trigger() {
        let snips = vec![("my email".to_string(), "a@b.com".to_string())];
        assert_eq!(
            expand_snippets("send it to my email please", &snips),
            "send it to a@b.com please"
        );
        // Word boundaries: no match inside another word.
        assert_eq!(
            expand_snippets("my emailx", &snips),
            "my emailx"
        );
    }
}
