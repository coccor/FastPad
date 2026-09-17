//! HTML inside Markdown: a small error-tolerant tokenizer and GitHub's sanitizer rules. Nothing
//! here fails; markup that is not a valid tag stays text, as GitHub shows it.

use crate::preview::html_entities;
use crate::preview::links::is_safe_url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    End {
        name: String,
    },
    /// Text with character references decoded.
    Text(String),
    /// A comment; its content is discarded.
    Comment,
}

/// Tags the preview renders. Every other tag is dropped and its text kept, unless it is in
/// `REMOVED_WITH_CONTENT`.
const ALLOWED_TAGS: &[&str] = &[
    "a", "abbr", "b", "bdo", "br", "cite", "code", "del", "details", "dfn", "div", "em", "h1",
    "h2", "h3", "h4", "h5", "h6", "h7", "h8", "hr", "i", "img", "ins", "kbd", "mark", "p",
    "picture", "q", "s", "samp", "small", "source", "span", "strike", "strong", "sub", "summary",
    "sup", "time", "tt", "var", "wbr",
];

/// Elements GitHub removes together with everything inside them.
const REMOVED_WITH_CONTENT: &[&str] = &[
    "button", "form", "iframe", "math", "noscript", "object", "script", "select", "style", "svg",
    "template", "textarea",
];

const ALLOWED_ATTRIBUTES: &[&str] = &[
    "align", "alt", "height", "href", "id", "media", "name", "open", "src", "srcset", "title",
    "width",
];

pub fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

pub fn tokenize(html: &str) -> Vec<Token> {
    let bytes = html.as_bytes();
    let mut tokens = Vec::new();
    let mut text_start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let parsed = if html[index..].starts_with("<!--") {
            let end = html[index + 4..]
                .find("-->")
                .map_or(bytes.len(), |found| index + 4 + found + 3);
            Some((Token::Comment, end))
        } else {
            parse_tag(html, index)
        };
        match parsed {
            Some((token, end)) => {
                push_text(&mut tokens, &html[text_start..index]);
                tokens.push(token);
                index = end;
                text_start = end;
            }
            None => index += 1,
        }
    }
    push_text(&mut tokens, &html[text_start..]);
    tokens
}

fn push_text(tokens: &mut Vec<Token>, raw: &str) {
    if !raw.is_empty() {
        tokens.push(Token::Text(decode_entities(raw)));
    }
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b':'
}

/// Parses the tag starting at `html[start] == '<'`. `None` when it is not a complete tag: the `<`
/// is then text.
fn parse_tag(html: &str, start: usize) -> Option<(Token, usize)> {
    let bytes = html.as_bytes();
    let mut index = start + 1;
    let closing = bytes.get(index) == Some(&b'/');
    if closing {
        index += 1;
    }
    if !bytes.get(index)?.is_ascii_alphabetic() {
        return None;
    }
    let name_start = index;
    while index < bytes.len() && is_name_byte(bytes[index]) {
        index += 1;
    }
    let name = html[name_start..index].to_ascii_lowercase();
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut self_closing = false;
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        match *bytes.get(index)? {
            b'>' => {
                index += 1;
                break;
            }
            b'/' => {
                index += 1;
                if bytes.get(index) == Some(&b'>') {
                    self_closing = true;
                    index += 1;
                    break;
                }
            }
            _ => {
                let attr_start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'=' | b'>' | b'/')
                {
                    index += 1;
                }
                if index == attr_start {
                    // A stray `=` where a name belongs.
                    index += 1;
                    continue;
                }
                let attr_name = html[attr_start..index].to_ascii_lowercase();
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                let mut value = String::new();
                if bytes.get(index) == Some(&b'=') {
                    index += 1;
                    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                        index += 1;
                    }
                    match *bytes.get(index)? {
                        quote @ (b'"' | b'\'') => {
                            let value_start = index + 1;
                            let end = value_start + html[value_start..].find(quote as char)?;
                            value = decode_entities(&html[value_start..end]);
                            index = end + 1;
                        }
                        _ => {
                            let value_start = index;
                            while index < bytes.len()
                                && !bytes[index].is_ascii_whitespace()
                                && bytes[index] != b'>'
                            {
                                index += 1;
                            }
                            value = decode_entities(&html[value_start..index]);
                        }
                    }
                }
                if !attrs.iter().any(|(existing, _)| *existing == attr_name) {
                    attrs.push((attr_name, value));
                }
            }
        }
    }
    let token = if closing {
        Token::End { name }
    } else {
        Token::Start {
            name,
            attrs,
            self_closing,
        }
    };
    Some((token, index))
}

pub fn decode_entities(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    let mut output = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        output.push_str(&rest[..amp]);
        rest = &rest[amp..];
        match decode_reference(rest) {
            Some((character, used)) => {
                output.push(character);
                rest = &rest[used..];
            }
            None => {
                output.push('&');
                rest = &rest[1..];
            }
        }
    }
    output.push_str(rest);
    output
}

/// Decodes the reference at the start of `reference` (which begins with `&`), returning the
/// character and the bytes it used including the `;`. Unknown or malformed references are `None`
/// and stay literal; numeric references outside Unicode decode to U+FFFD.
fn decode_reference(reference: &str) -> Option<(char, usize)> {
    // The longest HTML 4.01 name is 8 bytes; bounding the search keeps text full of `&` linear.
    let end = reference.bytes().take(34).position(|byte| byte == b';')?;
    let body = &reference[1..end];
    let character = if let Some(number) = body.strip_prefix('#') {
        let (digits, radix) = match number.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16),
            None => (number, 10),
        };
        if digits.is_empty() || !digits.chars().all(|digit| digit.is_digit(radix)) {
            return None;
        }
        u32::from_str_radix(digits, radix)
            .ok()
            .filter(|code| *code != 0)
            .and_then(char::from_u32)
            .unwrap_or('\u{FFFD}')
    } else {
        html_entities::lookup(body)?
    };
    Some((character, end + 1))
}

/// GitHub's HTML sanitizer as a token filter. It is stateful because removed elements
/// (`<script>…</script>`) can span many tokens, and in inline HTML many Markdown events.
#[derive(Debug, Default)]
pub struct Sanitizer {
    /// The element being removed and how deeply it is nested inside itself.
    removing: Option<(String, u32)>,
}

impl Sanitizer {
    pub fn filter(&mut self, token: Token) -> Option<Token> {
        if let Some((removed, depth)) = &mut self.removing {
            match &token {
                Token::Start {
                    name,
                    self_closing: false,
                    ..
                } if name == removed => *depth += 1,
                Token::End { name } if name == removed => {
                    *depth -= 1;
                    if *depth == 0 {
                        self.removing = None;
                    }
                }
                _ => {}
            }
            return None;
        }
        match token {
            Token::Start {
                name,
                attrs,
                self_closing,
            } => {
                if REMOVED_WITH_CONTENT.contains(&name.as_str()) {
                    if !self_closing {
                        self.removing = Some((name, 1));
                    }
                    None
                } else if ALLOWED_TAGS.contains(&name.as_str()) {
                    let attrs = attrs
                        .into_iter()
                        .filter(|(attr_name, value)| {
                            ALLOWED_ATTRIBUTES.contains(&attr_name.as_str())
                                && (!matches!(attr_name.as_str(), "href" | "src" | "srcset")
                                    || is_safe_url(value))
                        })
                        .collect();
                    Some(Token::Start {
                        name,
                        attrs,
                        self_closing,
                    })
                } else {
                    None
                }
            }
            Token::End { name } => ALLOWED_TAGS
                .contains(&name.as_str())
                .then_some(Token::End { name }),
            Token::Text(text) => Some(Token::Text(text)),
            Token::Comment => None,
        }
    }

    /// True inside a removed element: Markdown text and code arriving between inline HTML events
    /// belong to it and are dropped too.
    pub fn is_removing(&self) -> bool {
        self.removing.is_some()
    }

    pub fn reset(&mut self) {
        self.removing = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(name: &str, attrs: &[(&str, &str)]) -> Token {
        Token::Start {
            name: name.into(),
            attrs: attrs
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
            self_closing: false,
        }
    }

    fn end(name: &str) -> Token {
        Token::End { name: name.into() }
    }

    fn text(value: &str) -> Token {
        Token::Text(value.into())
    }

    #[test]
    fn tags_text_and_nesting() {
        assert_eq!(
            tokenize(r#"<div align="center">Hi <b>there</b></div>"#),
            vec![
                start("div", &[("align", "center")]),
                text("Hi "),
                start("b", &[]),
                text("there"),
                end("b"),
                end("div"),
            ]
        );
    }

    #[test]
    fn attribute_forms_and_case() {
        assert_eq!(
            tokenize("<IMG SRC='a.png' width=96 Alt=\"x y\" hidden>"),
            vec![start(
                "img",
                &[
                    ("src", "a.png"),
                    ("width", "96"),
                    ("alt", "x y"),
                    ("hidden", "")
                ]
            )]
        );
        let br = Token::Start {
            name: "br".into(),
            attrs: Vec::new(),
            self_closing: true,
        };
        assert_eq!(tokenize("<br/><br />"), vec![br.clone(), br]);
        assert_eq!(
            tokenize(r#"<a href="x" href="y">"#),
            vec![start("a", &[("href", "x")])]
        );
    }

    #[test]
    fn invalid_markup_stays_text() {
        for source in [
            "a < b",
            "<3",
            "x <div",
            r#"<a href="unterminated>t"#,
            "</ div>",
            "<!DOCTYPE html>",
        ] {
            assert_eq!(tokenize(source), vec![text(source)], "{source}");
        }
    }

    #[test]
    fn comments_are_discarded_even_when_unterminated() {
        assert_eq!(
            tokenize("a<!-- x > y -->b"),
            vec![text("a"), Token::Comment, text("b")]
        );
        assert_eq!(tokenize("a<!-- open"), vec![text("a"), Token::Comment]);
    }

    #[test]
    fn character_references_decode_in_text_and_attributes() {
        assert_eq!(
            tokenize("&copy; &#169; &#xA9; &#XA9; &nbsp;"),
            vec![text("© © © © \u{A0}")]
        );
        assert_eq!(
            tokenize("&unknown; &amp &;"),
            vec![text("&unknown; &amp &;")]
        );
        assert_eq!(
            tokenize("&#0;&#xD800;&#x110000;&#99999999999;"),
            vec![text("\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}")]
        );
        assert_eq!(
            tokenize(r#"<p title="a &amp; b">"#),
            vec![start("p", &[("title", "a & b")])]
        );
        assert_eq!(decode_entities("&#x;&#;&#12a;"), "&#x;&#;&#12a;");
    }

    fn sanitized(source: &str) -> Vec<Token> {
        let mut sanitizer = Sanitizer::default();
        tokenize(source)
            .into_iter()
            .filter_map(|token| sanitizer.filter(token))
            .collect()
    }

    #[test]
    fn dangerous_elements_are_removed_with_their_content() {
        assert_eq!(
            sanitized("a<script>alert(1)</script>b"),
            vec![text("a"), text("b")]
        );
        assert_eq!(sanitized("<svg><svg></svg>x</svg>y"), vec![text("y")]);
        assert_eq!(sanitized("<style/>kept"), vec![text("kept")]);
    }

    #[test]
    fn unknown_tags_are_dropped_and_their_text_kept() {
        assert_eq!(
            sanitized("<center>hi</center><u>x</u>"),
            vec![text("hi"), text("x")]
        );
    }

    #[test]
    fn only_allowlisted_attributes_and_safe_urls_survive() {
        assert_eq!(
            sanitized(r#"<a href="https://x.dev" style="color:red" onclick="f()">"#),
            vec![start("a", &[("href", "https://x.dev")])]
        );
        assert_eq!(
            sanitized(r#"<a href="javascript:alert(1)" title="t">"#),
            vec![start("a", &[("title", "t")])]
        );
        assert_eq!(
            sanitized(r#"<img src="data:image/png;base64,AA" alt="a">"#),
            vec![start("img", &[("alt", "a")])]
        );
        assert_eq!(sanitized("<!-- note -->"), vec![]);
    }
}
