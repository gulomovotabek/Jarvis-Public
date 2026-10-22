pub fn markdown_like_to_telegram_html(input: &str) -> String {
    let escaped = escape_html(input);
    apply_double_asterisk_bold(&escaped)
}

pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn apply_double_asterisk_bold(input: &str) -> String {
    if !input.contains("**") {
        return input.to_string();
    }

    let mut out = String::new();
    for (idx, part) in input.split("**").enumerate() {
        if idx % 2 == 1 {
            out.push_str("<b>");
            out.push_str(part);
            out.push_str("</b>");
        } else {
            out.push_str(part);
        }
    }
    out
}
