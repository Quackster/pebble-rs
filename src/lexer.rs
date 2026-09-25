// Port of the lexer of the external `io.pebbletemplates:pebble` template
// engine (Pebble 3.1.5), implemented inline since the library is not in the
// repo. Splits template source into raw text, `{{ ... }}` expressions and
// `{% ... %}` tags (`{# ... #}` comments are dropped).
#[derive(Debug, PartialEq, Eq)]
pub enum Token<'s> {
    Text(&'s str),
    Expr(&'s str),
    Tag(&'s str),
}

/// The byte spans of `{# ... #}` comments (an unterminated comment runs to
/// the end); these are excluded from text nodes.
fn comment_spans(src: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    while let Some(rel) = src[start..].find("{#") {
        let c = start + rel;
        match src[c + 2..].find("#}") {
            Some(end) => {
                spans.push((c, c + 2 + end + 2));
                start = c + 2 + end + 2;
            }
            None => {
                spans.push((c, src.len()));
                break;
            }
        }
    }
    spans
}

/// Push the text in `src[start..end]` as one node, minus the comment spans.
fn push_text<'a>(
    out: &mut Vec<Token<'a>>,
    src: &'a str,
    start: usize,
    end: usize,
    spans: &[(usize, usize)],
) {
    if start >= end {
        return;
    }
    let mut pos = start;
    for &(cs, ce) in spans {
        if ce <= start || cs >= end {
            continue;
        }
        let s = cs.max(start);
        let e = ce.min(end);
        if s > pos {
            out.push(Token::Text(&src[pos..s]));
        }
        pos = e;
    }
    if pos < end {
        out.push(Token::Text(&src[pos..end]));
    }
}

pub fn tokenize(src: &str) -> Vec<Token<'_>> {
    let spans = comment_spans(src);
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut last = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' {
            match bytes.get(i + 1) {
                Some(b'{') => match src[i + 2..].find("}}") {
                    Some(end) => {
                        push_text(&mut out, src, last, i, &spans);
                        out.push(Token::Expr(&src[i + 2..i + 2 + end]));
                        i = i + 2 + end + 2;
                        last = i;
                        continue;
                    }
                    // Unterminated `{{` — keep as literal text.
                    None => i += 1,
                },
                Some(b'%') => match src[i + 2..].find("%}") {
                    Some(end) => {
                        push_text(&mut out, src, last, i, &spans);
                        out.push(Token::Tag(&src[i + 2..i + 2 + end]));
                        i = i + 2 + end + 2;
                        last = i;
                        continue;
                    }
                    // Unterminated `{%` — keep as literal text.
                    None => i += 1,
                },
                // `{# ... #}` — skipped as text is flushed around the span.
                Some(b'#') => {
                    i = match src[i + 2..].find("#}") {
                        Some(end) => i + 2 + end + 2,
                        None => src.len(),
                    };
                    continue;
                }
                _ => i += 1,
            }
        } else {
            i += 1;
        }
    }
    push_text(&mut out, src, last, src.len(), &spans);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_all_three_kinds() {
        let tokens = tokenize("a {{ x }} b {% if y %} c {# d #} e");
        // The comment drops, leaving two adjacent text nodes whose concatenation
        // is ` c  e` (they cannot be merged into one borrowed slice).
        assert_eq!(
            tokens,
            vec![
                Token::Text("a "),
                Token::Expr(" x "),
                Token::Text(" b "),
                Token::Tag(" if y "),
                Token::Text(" c "),
                Token::Text(" e"),
            ]
        );
    }

    #[test]
    fn comment_between_text_is_dropped() {
        let tokens = tokenize("x{# c #}y");
        let texts: Vec<&str> = tokens
            .iter()
            .filter_map(|t| if let Token::Text(s) = t { Some(*s) } else { None })
            .collect();
        assert_eq!(texts, vec!["x", "y"]);
    }

    #[test]
    fn unterminated_opening_stays_text() {
        let tokens = tokenize("a {{ x");
        assert_eq!(tokens, vec![Token::Text("a {{ x")]);
    }
}
