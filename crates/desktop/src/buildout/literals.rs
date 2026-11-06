/// Read a JavaScript string literal as data. Convert its escapes to a JSON
/// string, letting serde handle Unicode including surrogate pairs.
fn string_literal(input: &str) -> Option<(String, usize)> {
    let mut chars = input.chars();
    let quote = chars.next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let mut json = String::from("\"");
    while let Some(ch) = chars.next() {
        match ch {
            ch if ch == quote => {
                json.push('"');
                return Some((
                    serde_json::from_str(&json).ok()?,
                    input.len() - chars.as_str().len(),
                ));
            }
            '"' => json.push_str("\\\""),
            '\\' => match chars.next()? {
                '\'' => json.push('\''),
                'x' => {
                    json.push_str("\\u00");
                    json.push(chars.next()?);
                    json.push(chars.next()?);
                }
                escaped => {
                    json.push('\\');
                    json.push(escaped);
                }
            },
            other => json.push(other),
        }
    }
    None
}

/// The small news list can be emitted as plain JS objects with bare keys.
/// Copy only literal tokens through the closing bracket. serde_json checks the
/// resulting structure and rejects expressions, calls and trailing junk.
fn object_array(mut input: &str) -> Option<serde_json::Value> {
    let mut json = String::new();
    let mut depth = 0usize;
    while let Some(ch) = input.chars().next() {
        match ch {
            '"' | '\'' => {
                let (text, used) = string_literal(input)?;
                json.push_str(&serde_json::to_string(&text).ok()?);
                input = &input[used..];
                continue;
            }
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    json.push(']');
                    return serde_json::from_str(&json).ok();
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let end = input
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(input.len());
                let (word, rest) = input.split_at(end);
                if rest.trim_start().starts_with(':') {
                    json.push('"');
                    json.push_str(word);
                    json.push('"');
                } else {
                    json.push_str(word);
                }
                input = rest;
                continue;
            }
            _ => {}
        }
        json.push(ch);
        input = &input[ch.len_utf8()..];
    }
    None
}

pub(super) fn arrays(script: &str) -> impl Iterator<Item = serde_json::Value> + '_ {
    script.match_indices(".exports=").filter_map(|(i, marker)| {
        let input = script[i + marker.len()..].trim_start();
        if let Some(literal) = input.strip_prefix("JSON.parse(") {
            let (json, _) = string_literal(literal.trim_start())?;
            serde_json::from_str(&json).ok()
        } else if input.starts_with("[{") {
            object_array(input)
        } else {
            None
        }
    })
}
