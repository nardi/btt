//! Reading JavaScript object literals, as DWR replies carry them, into JSON values.
//!
//! A literal differs from JSON in its keys, which need no quotes. Strings use double quotes
//! with JSON escapes, and values are objects, arrays, strings, numbers, `true`, `false` or
//! `null`.

use serde_json::{Map, Number, Value};

/// Reads the literal that starts at the beginning of `text`, and returns it with the rest of
/// the text after it, or `None` when it is not a literal of the supported form.
pub fn parse(text: &str) -> Option<(Value, &str)> {
    let mut reader = Reader { text, position: 0 };
    let value = reader.value()?;
    Some((value, &text[reader.position..]))
}

/// A position in the text of a literal.
struct Reader<'a> {
    /// The text.
    text: &'a str,
    /// The byte position of the next character to read.
    position: usize,
}

impl Reader<'_> {
    /// Returns the rest of the text.
    fn rest(&self) -> &str {
        &self.text[self.position..]
    }

    /// Skips white space.
    fn skip_space(&mut self) {
        let rest = self.rest();
        self.position += rest.len() - rest.trim_start().len();
    }

    /// Skips white space and returns the next byte without reading it.
    fn peek(&mut self) -> Option<u8> {
        self.skip_space();
        self.rest().bytes().next()
    }

    /// Reads one byte that must be `expected`.
    fn expect(&mut self, expected: u8) -> Option<()> {
        (self.peek()? == expected).then(|| self.position += 1)
    }

    /// Reads a value.
    fn value(&mut self) -> Option<Value> {
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(Value::String),
            _ => self.word(),
        }
    }

    /// Reads an object with quoted or bare keys.
    fn object(&mut self) -> Option<Value> {
        self.expect(b'{')?;
        let mut object = Map::new();
        if self.peek()? == b'}' {
            self.position += 1;
            return Some(Value::Object(object));
        }
        loop {
            let key = if self.peek()? == b'"' {
                self.string()?
            } else {
                let length = self.rest().find(|character: char| {
                    !(character.is_alphanumeric() || "_$".contains(character))
                })?;
                let key = self.rest()[..length].to_owned();
                self.position += length;
                key
            };
            self.expect(b':')?;
            let value = self.value()?;
            object.insert(key, value);
            match self.peek()? {
                b',' => self.position += 1,
                b'}' => {
                    self.position += 1;
                    return Some(Value::Object(object));
                }
                _ => return None,
            }
        }
    }

    /// Reads an array.
    fn array(&mut self) -> Option<Value> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        if self.peek()? == b']' {
            self.position += 1;
            return Some(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            match self.peek()? {
                b',' => self.position += 1,
                b']' => {
                    self.position += 1;
                    return Some(Value::Array(items));
                }
                _ => return None,
            }
        }
    }

    /// Reads a string in double quotes with JSON escapes.
    fn string(&mut self) -> Option<String> {
        let start = self.position;
        let bytes = self.text.as_bytes();
        let mut end = start + 1;
        while *bytes.get(end)? != b'"' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        self.position = end + 1;
        serde_json::from_str(&self.text[start..=end]).ok()
    }

    /// Reads a number, `true`, `false` or `null`.
    fn word(&mut self) -> Option<Value> {
        let start = self.position;
        let rest = &self.text[start..];
        let length = rest
            .find(|character: char| !(character.is_alphanumeric() || ".+-".contains(character)))
            .unwrap_or(rest.len());
        let word = &self.text[start..start + length];
        self.position = start + length;
        match word {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            "null" => Some(Value::Null),
            _ => word
                .parse::<f64>()
                .ok()
                .and_then(Number::from_f64)
                .map(Value::Number),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn literals_with_bare_keys_read_as_json() {
        let (value, rest) =
            parse(r#"{a:1,"b":[true,null,"x\"yÁ"],c:{d:-2.5,e:[]},f:{}});"#).unwrap();
        assert_eq!(
            value,
            json!({ "a": 1.0, "b": [true, null, "x\"yÁ"], "c": { "d": -2.5, "e": [] }, "f": {} })
        );
        assert_eq!(rest, ");");
    }

    #[test]
    fn a_broken_literal_is_rejected() {
        assert!(parse("{a:1,").is_none());
        assert!(parse("{a 1}").is_none());
    }
}
