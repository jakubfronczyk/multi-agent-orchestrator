//! An HTML page as a reader sees it: its title and its text, without markup, scripts or styles.
//! A small scanner, not a parser: it only has to drop what is never shown and keep the rest.

/// Elements whose content is never shown as text.
const HIDDEN: [&str; 6] = ["script", "style", "noscript", "svg", "template", "iframe"];
/// Elements that start a new line of text.
const BLOCKS: [&str; 24] = [
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "tr",
    "td",
    "th",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "section",
    "article",
    "header",
    "footer",
    "blockquote",
    "pre",
    "hr",
    "dd",
];

pub(super) struct Page {
    pub title: Option<String>,
    pub text: String,
}

/// Lowercasing is ASCII only, so every byte offset into `lower` is one into `html`.
pub(super) fn readable(html: &str) -> Page {
    let lower = html.to_ascii_lowercase();
    let mut title = None;
    let mut text = String::new();
    let mut at = 0;
    while let Some(open) = lower[at..].find('<').map(|i| at + i) {
        text.push_str(&html[at..open]);
        if lower[open..].starts_with("<!--") {
            at = lower[open..]
                .find("-->")
                .map_or(html.len(), |i| open + i + 3);
            continue;
        }
        let Some(close) = lower[open..].find('>').map(|i| open + i) else {
            at = html.len();
            break;
        };
        let name: String = lower[open + 1..close]
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        at = close + 1;
        if HIDDEN.contains(&name.as_str()) && !lower[open + 1..].starts_with('/') {
            at = end_of(&lower, &name, at);
        } else if name == "title" && !lower[open + 1..].starts_with('/') {
            let end = lower[at..].find("</title").map_or(html.len(), |i| at + i);
            title = Some(collapse(&decode(&html[at..end]))).filter(|t| !t.is_empty());
            at = end_of(&lower, "title", at);
        } else if BLOCKS.contains(&name.as_str()) {
            text.push('\n');
        } else {
            text.push(' ');
        }
    }
    text.push_str(&html[at.min(html.len())..]);
    Page {
        title,
        text: lines(&decode(&text)),
    }
}

/// Just past `</name ...>`, or the end of the page.
fn end_of(lower: &str, name: &str, from: usize) -> usize {
    let closing = format!("</{name}");
    match lower[from..].find(&closing) {
        Some(i) => lower[from + i..]
            .find('>')
            .map_or(lower.len(), |j| from + i + j + 1),
        None => lower.len(),
    }
}

/// The common named entities and every numeric one; anything else is kept as written.
fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let entity = rest[1..]
            .find(';')
            .filter(|end| *end <= 10)
            .map(|end| &rest[1..=end]);
        let decoded = entity.and_then(|name| match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        });
        match (entity, decoded) {
            (Some(name), Some(c)) => {
                out.push(c);
                rest = &rest[name.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One space between words, one line per block, no empty lines.
fn lines(text: &str) -> String {
    text.lines()
        .map(collapse)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_keeps_its_title_and_text_and_drops_what_is_never_shown() {
        let html = r#"<!DOCTYPE html><html><head><title>SQLite &amp; you</title>
            <style>body { color: red }</style><script>var x = "<p>not text</p>";</script></head>
            <body><!-- hidden --><h1>About</h1><p>Developed by D.&nbsp;Richard Hipp &#8212; public domain.</p>
            <ul><li>One</li><li>Two</li></ul><SCRIPT>alert(1)</SCRIPT>Żółw &#x2713;</body></html>"#;
        let page = readable(html);
        assert_eq!(page.title.as_deref(), Some("SQLite & you"));
        assert_eq!(
            page.text,
            "About\nDeveloped by D. Richard Hipp \u{2014} public domain.\nOne\nTwo\nŻółw \u{2713}"
        );
    }

    #[test]
    fn broken_markup_and_stray_ampersands_never_panic() {
        for html in [
            "<p>unclosed",
            "<script>never closed",
            "a & b &unknown; &#xZZ; <",
            "<!-- open",
        ] {
            let _ = readable(html);
        }
        assert_eq!(readable("a & b").text, "a & b");
    }
}
