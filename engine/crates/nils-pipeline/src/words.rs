// SPDX-License-Identifier: AGPL-3.0-only

//! The command line a container runs. A descriptor writes it as one string,
//! as Boutiques does; the runner splits it into words the way a shell does
//! (quotes and backslashes, nothing expanded, no shell run), then replaces the
//! value-keys word by word. A word that is a value-key alone becomes the words
//! the key stands for, none for an unset optional parameter; a key inside a
//! longer word is replaced by its words joined with a space, so a key inside
//! a quoted `bash -c '...'` script reaches the script.

/// Split a command line into words as a POSIX shell would, without
/// expanding anything.
pub fn split(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("a single quote is not closed".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => word.push(c),
                            Some('\n') => {}
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("a double quote is not closed".into()),
                        },
                        Some(c) => word.push(c),
                        None => return Err("a double quote is not closed".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some('\n') => {}
                    Some(c) => word.push(c),
                    None => return Err("the line ends in a backslash".into()),
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    if words.is_empty() {
        return Err("the command line is empty".into());
    }
    Ok(words)
}

/// Replace the keys in the words: a word that is a key alone becomes the
/// key's words, one inside a longer word is replaced by them joined.
pub fn substitute(words: &[String], keys: &[(String, Vec<String>)]) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    for w in words {
        if let Some((_, expansion)) = keys.iter().find(|(k, _)| k == w) {
            out.extend(expansion.iter().cloned());
            continue;
        }
        let mut text = w.clone();
        for (k, expansion) in keys {
            if text.contains(k.as_str()) {
                text = text.replace(k.as_str(), &expansion.join(" "));
            }
        }
        out.push(text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_splits_as_a_shell_splits_it() {
        assert_eq!(
            split(r#"bash -c 'a "b" $c' "d \"e\" \$f" g\ h  "#).unwrap(),
            ["bash", "-c", r#"a "b" $c"#, r#"d "e" $f"#, "g h"]
        );
        assert_eq!(split("''").unwrap(), [""]);
        assert!(split("a 'b").is_err());
        assert!(split("a \"b").is_err());
        assert!(split("   ").is_err());
    }

    #[test]
    fn a_key_alone_is_words_and_inside_a_word_is_text() {
        let words = split("run [L] x=[L] [NONE] [A]/f").unwrap();
        let keys = vec![
            ("[L]".to_string(), vec!["1".to_string(), "2".to_string()]),
            ("[NONE]".to_string(), vec![]),
            ("[A]".to_string(), vec!["/input".to_string()]),
        ];
        assert_eq!(
            substitute(&words, &keys),
            ["run", "1", "2", "x=1 2", "/input/f"]
        );
    }
}
