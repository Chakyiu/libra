/// Local models emit markdown fences, prose preambles and trailing commas.
/// This recovers a JSON value from that mess without pulling in a dependency.
pub fn extract_json(raw: &str) -> anyhow::Result<serde_json::Value> {
    let s = raw.trim();
    let mut candidates: Vec<String> = Vec::with_capacity(4);
    candidates.push(s.to_string());

    // ```json ... ``` fences
    if let Some(start) = s.find("```") {
        let rest = &s[start + 3..];
        let body = match rest.find("```") {
            Some(end) => &rest[..end],
            None => rest,
        };
        let body = body.trim_start_matches("json").trim();
        candidates.push(body.to_string());
    }

    // First balanced object/array slice.
    if let Some(begin) = s.find(['{', '[']) {
        candidates.push(balanced(&s[begin..]).to_string());
    }

    for c in candidates {
        if c.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&c) {
            return Ok(v);
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&strip_trailing_commas(&c)) {
            return Ok(v);
        }
    }
    anyhow::bail!("no JSON found in model output: {}", &s.chars().take(240).collect::<String>())
}

fn balanced(s: &str) -> &str {
    let bytes = s.as_bytes();
    let Some(open) = bytes.first().copied() else { return s };
    let close = match open {
        b'{' => b'}',
        b'[' => b']',
        _ => return s,
    };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' if in_str => escaped = true,
            b'"' => in_str = !in_str,
            _ if in_str => {}
            c if *c == open => depth += 1,
            c if *c == close => {
                depth -= 1;
                if depth == 0 {
                    return &s[..=i];
                }
            }
            _ => {}
        }
    }
    s
}

fn strip_trailing_commas(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut escaped = false;
    let chars: Vec<char> = s.chars().collect();
    for i in 0..chars.len() {
        let c = chars[i];
        if escaped {
            escaped = false;
            out.push(c);
            continue;
        }
        if c == '\\' && in_str {
            escaped = true;
            out.push(c);
            continue;
        }
        if c == '"' {
            in_str = !in_str;
        }
        if c == ',' && !in_str {
            if let Some(next) = chars.get(i + 1) {
                if *next == '}' || *next == ']' {
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_and_prose() {
        let v = extract_json("Sure! ```json\n{\"a\": 1, \"b\": [1,2,]}\n``` hope that helps").unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn nested_braces_in_strings() {
        let v = extract_json("{\"plan\": \"use {braces} here\", \"done\": false}").unwrap();
        assert_eq!(v["done"], false);
    }

    #[test]
    fn rejects_garbage() {
        assert!(extract_json("I have no idea").is_err());
    }
}
