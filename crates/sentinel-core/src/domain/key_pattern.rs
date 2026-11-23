//! Pure syntax model for bounded key-pattern expansion.

use serde::{Deserialize, Serialize};

use crate::adapters::key_expr::{MAX_KEY_EXPR_DEPTH, MAX_KEY_EXPR_LEN};

pub const MAX_KEY_PATTERN_CANDIDATES: usize = 256;
pub const MAX_KEY_PATTERN_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyPatternToken {
    Literal(String),
    SymbolPrefix(String),
    AddressWildcard,
    Composite(Vec<KeyPatternToken>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyPattern {
    Single(KeyPatternToken),
    Composite(Vec<KeyPatternToken>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPatternCandidateInput {
    pub addresses: Vec<String>,
    pub symbol_suffixes: Vec<String>,
}

impl Default for KeyPatternCandidateInput {
    fn default() -> Self {
        Self {
            addresses: Vec::new(),
            symbol_suffixes: Vec::from([
                "_1".to_string(),
                "_2".to_string(),
                "_3".to_string(),
                "_4".to_string(),
            ]),
        }
    }
}

pub fn parse_key_pattern(input: &str) -> Result<KeyPattern, (usize, String)> {
    if input.trim().is_empty() {
        return Err((0, "Empty key pattern".to_string()));
    }
    if input.len() > MAX_KEY_PATTERN_LEN || input.len() > MAX_KEY_EXPR_LEN {
        return Err((0, "Key pattern exceeds 1024 characters".to_string()));
    }

    let mut parser = PatternParser::new(input);
    let token = parser.parse(0)?;
    parser.skip_whitespace();
    if !parser.is_eof() {
        return Err((
            parser.pos,
            format!("Unexpected trailing input '{}'", parser.remaining()),
        ));
    }

    match token {
        KeyPatternToken::Composite(tokens) => Ok(KeyPattern::Composite(tokens)),
        token => Ok(KeyPattern::Single(token)),
    }
}

struct PatternParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> PatternParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.pos).copied()
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.peek() {
            if byte.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.input[self.pos..].starts_with(prefix)
    }

    fn parse(&mut self, depth: usize) -> Result<KeyPatternToken, (usize, String)> {
        if depth > MAX_KEY_EXPR_DEPTH {
            return Err((
                self.pos,
                format!("Key pattern exceeds maximum nesting depth of {MAX_KEY_EXPR_DEPTH}"),
            ));
        }
        self.skip_whitespace();
        if self.starts_with("vec[") {
            let vec_pos = self.pos;
            self.pos += 4;
            self.parse_composite(depth, vec_pos)
        } else {
            self.parse_token()
        }
    }

    fn parse_composite(
        &mut self,
        depth: usize,
        vec_pos: usize,
    ) -> Result<KeyPatternToken, (usize, String)> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace();
            if self.is_eof() {
                return Err((
                    vec_pos,
                    "Unbalanced brackets in key pattern: missing ']'".to_string(),
                ));
            }
            if self.peek() == Some(b']') {
                self.pos += 1;
                break;
            }

            tokens.push(self.parse(depth + 1)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                Some(byte) => {
                    return Err((
                        self.pos,
                        format!(
                            "Expected ',' or ']' in key pattern, found '{}'",
                            byte as char
                        ),
                    ));
                }
                None => {
                    return Err((
                        vec_pos,
                        "Unbalanced brackets in key pattern: missing ']'".to_string(),
                    ));
                }
            }
        }

        if tokens.is_empty() {
            return Err((
                vec_pos + 4,
                "Composite pattern must contain at least one element".to_string(),
            ));
        }
        Ok(KeyPatternToken::Composite(tokens))
    }

    fn parse_token(&mut self) -> Result<KeyPatternToken, (usize, String)> {
        let start = self.pos;
        while let Some(byte) = self.peek() {
            if byte == b',' || byte == b']' || byte.is_ascii_whitespace() {
                break;
            }
            self.pos += 1;
        }
        let token = &self.input[start..self.pos];
        if token.is_empty() {
            return Err((start, "Empty key pattern token".to_string()));
        }

        let Some(first_star) = token.find('*') else {
            return Ok(KeyPatternToken::Literal(token.to_string()));
        };

        for (offset, _) in token.match_indices('*') {
            let symbol_prefix = token.starts_with("sym:")
                && offset == token.len() - 1
                && offset > 4
                && is_valid_symbol_prefix(&token[4..offset]);
            let address_wildcard = token == "addr:*" && offset == token.len() - 1;
            if !symbol_prefix && !address_wildcard {
                return Err((
                    start + offset,
                    format!(
                        "Invalid wildcard at byte {} in '{token}': '*' is only valid as trailing sym:<prefix>* or as addr:*",
                        start + first_star
                    ),
                ));
            }
        }

        if token.starts_with("sym:") {
            return Ok(KeyPatternToken::SymbolPrefix(
                token[4..token.len() - 1].to_string(),
            ));
        }
        Ok(KeyPatternToken::AddressWildcard)
    }
}

fn is_valid_symbol_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && prefix.len() <= 32
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_prefix_composite_and_empty_rejection() {
        assert!(
            matches!(parse_key_pattern("sym:Balance*"), Ok(KeyPattern::Single(KeyPatternToken::SymbolPrefix(p))) if p == "Balance")
        );
        assert!(matches!(
            parse_key_pattern("vec[sym:Allow, addr:*]"),
            Ok(KeyPattern::Composite(_))
        ));
        assert!(parse_key_pattern("").is_err());
    }

    #[test]
    fn invalid_middle_wildcard_has_position() {
        let err = parse_key_pattern("sym:Bal*ance").unwrap_err();
        assert_eq!(err.0, 7);
    }

    #[test]
    fn invalid_address_wildcard_has_position() {
        let err = parse_key_pattern("addr:G*").unwrap_err();
        assert_eq!(err.0, 6);
    }

    #[test]
    fn nested_composite_patterns_are_accepted() {
        let parsed = parse_key_pattern("vec[sym:Allow, vec[u32:1, addr:*]]").expect("nested");
        assert!(matches!(
            parsed,
            KeyPattern::Composite(tokens)
                if matches!(tokens.last(), Some(KeyPatternToken::Composite(_)))
        ));
    }

    #[test]
    fn empty_composite_and_empty_prefix_are_rejected() {
        assert!(parse_key_pattern("vec[]").is_err());
        assert!(parse_key_pattern("sym:*").is_err());
    }

    #[test]
    fn excessive_length_and_nesting_are_rejected() {
        let too_long = format!("sym:{}", "a".repeat(MAX_KEY_PATTERN_LEN));
        assert!(parse_key_pattern(&too_long).is_err());

        let mut deeply_nested = "u32:1".to_string();
        for _ in 0..=MAX_KEY_EXPR_DEPTH {
            deeply_nested = format!("vec[{deeply_nested}]");
        }
        assert!(parse_key_pattern(&deeply_nested).is_err());
    }

    #[test]
    fn trailing_input_is_rejected() {
        let err = parse_key_pattern("sym:Balance* addr:*").unwrap_err();
        assert!(err.1.contains("Unexpected trailing input"));
    }
}
