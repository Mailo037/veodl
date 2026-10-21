use crate::error::Error;

struct Parser<'a> {
    text: &'a [char],
    index: usize,
}

type Fail = (String, usize);

impl Parser<'_> {
    fn fail<T>(&self, reason: &str) -> Result<T, Fail> {
        Err((reason.to_string(), self.index))
    }

    fn at(&self) -> Option<char> {
        self.text.get(self.index).copied()
    }

    fn space(&mut self) {
        while self.at().is_some_and(char::is_whitespace) {
            self.index += 1;
        }
    }

    fn string(&mut self) -> Result<(), Fail> {
        self.index += 1;
        while self.index < self.text.len() {
            let c = self.text[self.index];
            self.index += 1;
            if c == '"' {
                return Ok(());
            }
            if (c as u32) < 32 {
                self.index -= 1;
                return self.fail("Unescaped line break or control character inside a string.");
            }
            if c == '\\' {
                match self.at() {
                    Some('u') => {
                        self.index += 1;
                        for _ in 0..4 {
                            if !self.at().is_some_and(|c| c.is_ascii_hexdigit()) {
                                return self.fail("Invalid Unicode escape; use four hexadecimal digits after \\u.");
                            }
                            self.index += 1;
                        }
                    }
                    Some(escape) if "\"\\/bfnrt".contains(escape) => self.index += 1,
                    _ => return self.fail("Invalid escape sequence. For Windows paths, use forward slashes (G:/Videos) or double backslashes (G:\\\\Videos)."),
                }
            }
        }
        self.fail("Unterminated string; add a closing double quote.")
    }

    fn value(&mut self) -> Result<(), Fail> {
        self.space();
        let fence = "Markdown code fences are not valid JSON. Remove the lines containing ```json and ```.";
        match self.at() {
            Some('`') => return self.fail(fence),
            Some('"') => return self.string(),
            Some(open @ ('{' | '[')) => {
                let object = open == '{';
                let close = if object { '}' } else { ']' };
                self.index += 1;
                self.space();
                if self.at() == Some(close) {
                    self.index += 1;
                    return Ok(());
                }
                loop {
                    self.space();
                    if self.at() == Some('`') {
                        return self.fail(fence);
                    }
                    if object {
                        if self.at() != Some('"') {
                            return self.fail("Expected a property name in double quotes. Check for a trailing comma or a missing closing brace.");
                        }
                        self.string()?;
                        self.space();
                        if self.at() != Some(':') {
                            return self.fail("Expected a colon (:) after the property name.");
                        }
                        self.index += 1;
                    }
                    self.value()?;
                    self.space();
                    if self.at() == Some(close) {
                        self.index += 1;
                        return Ok(());
                    }
                    if self.at() != Some(',') {
                        return self.fail(&format!("Expected a comma (,) or closing {close}."));
                    }
                    self.index += 1;
                    self.space();
                    if self.at() == Some(close) {
                        return self.fail("Trailing commas are not allowed. Remove the comma before the closing bracket or brace.");
                    }
                }
            }
            _ => {}
        }
        let rest: String = self.text[self.index..].iter().take(64).collect();
        match crate::re!(r"^(?:true|false|null|-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)").find(&rest) {
            Some(literal) => {
                self.index += literal.as_str().chars().count();
                Ok(())
            }
            None => self.fail("Expected a JSON value: object, array, string, number, true, false or null."),
        }
    }
}

/// Diagnose syntax without echoing config values (which can include private
/// paths). `known` is a byte offset and reason already found by the comment stripper.
pub fn config_syntax_error(text: &str, file: &str, known: Option<(usize, String)>) -> Error {
    let chars: Vec<char> = text.chars().collect();
    let issue: Option<Fail> = match known {
        Some((byte, reason)) => Some((reason, text.get(..byte).map_or(chars.len(), |prefix| prefix.chars().count()))),
        None => {
            let mut parser = Parser { text: &chars, index: 0 };
            match parser.value() {
                Err(fail) => Some(fail),
                Ok(()) => {
                    parser.space();
                    (parser.index != chars.len())
                        .then(|| ("Unexpected content after the JSON value. Remove extra text or Markdown code fences.".to_string(), parser.index))
                }
            }
        }
    };
    let position = issue.as_ref().map_or(chars.len(), |(_, position)| *position).min(chars.len());
    let preceding = &chars[..position];
    let line = preceding.iter().filter(|c| **c == '\n').count() + 1;
    let column = position - preceding.iter().rposition(|c| *c == '\n').map_or(0, |index| index + 1) + 1;
    let reason = issue.map(|(reason, _)| reason).unwrap_or_else(|| "Invalid JSON syntax.".into());
    Error::msg(format!("The veo config file is not valid JSON: {file} (line {line}, column {column}). {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_common_mistakes() {
        let trailing = config_syntax_error("{\n  \"a\": 1,\n}", "c.json", None).message;
        assert!(trailing.contains("(line 3, column 1). Trailing commas"), "{trailing}");
        let path = config_syntax_error("{\"output\": \"D:\\Videos\"}", "c.json", None).message;
        assert!(path.contains("Invalid escape sequence"), "{path}");
        let fence = config_syntax_error("```json\n{}\n```", "c.json", None).message;
        assert!(fence.contains("Markdown code fences"), "{fence}");
    }
}
