//! Convert old `{placeholder}` syntax into Jinja2 syntax.
//!
//! upstream: src/documents/templating/utils.py@v3.4.2 (convert_format_str_to_template_format)

/// `\w` of Python `re` for str patterns.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `re.sub(r"(?<!\{)\{(\w*)\}(?!\})", r"{{ \1 }}", old_format)`.
pub fn convert_format_str_to_template_format(old_format: &str) -> String {
    let chars: Vec<char> = old_format.chars().collect();
    let mut out = String::with_capacity(old_format.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' && (i == 0 || chars[i - 1] != '{') {
            let mut j = i + 1;
            while j < chars.len() && is_word(chars[j]) {
                j += 1;
            }
            let next_ok = j + 1 >= chars.len() || chars[j + 1] != '}';
            if j < chars.len() && chars[j] == '}' && next_ok {
                let var: String = chars[i + 1..j].iter().collect();
                out.push_str("{{ ");
                out.push_str(&var);
                out.push_str(" }}");
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::convert_format_str_to_template_format as c;

    #[test]
    fn converts() {
        assert_eq!(c("{title} by {author}"), "{{ title }} by {{ author }}");
        assert_eq!(c("{{ title }}/{correspondent}"), "{{ title }}/{{ correspondent }}");
        assert_eq!(c("{% if x %}{title}{% endif %}"), "{% if x %}{{ title }}{% endif %}");
        assert_eq!(c("{}"), "{{  }}");
        assert_eq!(c("{created__year}"), "{{ created__year }}");
        assert_eq!(c("{% if x is None %}/{title]"), "{% if x is None %}/{title]");
        assert_eq!(c("Doc {created_year]"), "Doc {created_year]");
        assert_eq!(c("{a}{b}"), "{{ a }}{{ b }}");
    }
}
