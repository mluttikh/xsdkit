//! XSD regular expressions, translated to the `regex` crate.
//!
//! XSD patterns are **not** PCRE. The differences that matter:
//!
//! | XSD | Meaning | `regex` equivalent |
//! |---|---|---|
//! | whole pattern | implicitly anchored | `^(?:…)$` |
//! | `\i`, `\c` | XML name-start / name characters | explicit classes |
//! | `\p{IsBasicLatin}` | Unicode *block* | an explicit range |
//! | `[a-z-[aeiou]]` | class **subtraction** | difference computed here |
//! | `^`, `$` | ordinary characters, not anchors | escaped |
//! | `\d`, `\w`, `\s` | Unicode-aware, different membership | mapped |
//!
//! There are no backreferences and no lookaround in XSD, so nothing is lost
//! by targeting `regex` — and its linear-time guarantee removes catastrophic
//! backtracking as a denial-of-service vector, which a hand-written engine
//! would have to solve separately.

use regex::Regex;
use std::fmt;

/// Why an XSD pattern could not be compiled.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PatternError {
    pub pattern: String,
    pub reason: String,
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid XSD pattern `{}`: {}", self.pattern, self.reason)
    }
}

impl std::error::Error for PatternError {}

/// One restriction step's patterns, compiled.
///
/// Alternatives within a step are ORed, which is done by compiling them into
/// a single alternation rather than testing each in turn.
#[derive(Clone, Debug)]
pub struct PatternStep(Regex);

impl PatternStep {
    /// Compiles the alternatives declared at one restriction step.
    pub fn compile(alternatives: &[String]) -> Result<Self, PatternError> {
        let mut branches = Vec::with_capacity(alternatives.len());
        for a in alternatives {
            branches.push(format!("(?:{})", translate(a)?));
        }
        // Implicitly anchored: an XSD pattern matches the *whole* value.
        let joined = format!("^(?:{})$", branches.join("|"));
        Regex::new(&joined)
            .map(PatternStep)
            .map_err(|e| PatternError {
                pattern: alternatives.join("|"),
                reason: e.to_string(),
            })
    }

    pub fn is_match(&self, value: &str) -> bool {
        self.0.is_match(value)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Every pattern step in force on a type.
///
/// A value must satisfy **every** step — patterns AND across restriction
/// steps, even though they OR within one.
#[derive(Clone, Debug, Default)]
pub struct Patterns(Vec<PatternStep>);

impl Patterns {
    /// Compiles a [`crate::datatypes::FacetSet`]'s `patterns` field.
    pub fn compile(steps: &[Vec<String>]) -> Result<Self, PatternError> {
        steps
            .iter()
            .map(|s| PatternStep::compile(s))
            .collect::<Result<Vec<_>, _>>()
            .map(Patterns)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the value satisfies every step.
    pub fn is_match(&self, value: &str) -> bool {
        self.0.iter().all(|s| s.is_match(value))
    }

    /// The first step the value fails, for diagnostics.
    pub fn first_failure(&self, value: &str) -> Option<&PatternStep> {
        self.0.iter().find(|s| !s.is_match(value))
    }
}

impl FromIterator<PatternStep> for Patterns {
    fn from_iter<T: IntoIterator<Item = PatternStep>>(iter: T) -> Self {
        Patterns(iter.into_iter().collect())
    }
}

/// Translates one XSD pattern into `regex` syntax.
pub fn translate(pattern: &str) -> Result<String, PatternError> {
    let mut t = Translator {
        chars: pattern.chars().collect(),
        pos: 0,
        out: String::with_capacity(pattern.len() + 8),
        source: pattern,
    };
    t.regex()?;
    if t.pos != t.chars.len() {
        return Err(t.error("unbalanced `)`"));
    }
    Ok(t.out)
}

struct Translator<'a> {
    chars: Vec<char>,
    pos: usize,
    out: String,
    source: &'a str,
}

impl Translator<'_> {
    fn error(&self, reason: &str) -> PatternError {
        PatternError {
            pattern: self.source.to_string(),
            reason: format!("{reason} at offset {}", self.pos),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    /// `regExp ::= branch ( '|' branch )*`
    fn regex(&mut self) -> Result<(), PatternError> {
        loop {
            self.branch()?;
            match self.peek() {
                Some('|') => {
                    self.pos += 1;
                    self.out.push('|');
                }
                _ => return Ok(()),
            }
        }
    }

    fn branch(&mut self) -> Result<(), PatternError> {
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            self.piece()?;
        }
        Ok(())
    }

    /// `piece ::= atom quantifier?`
    fn piece(&mut self) -> Result<(), PatternError> {
        self.atom()?;
        match self.peek() {
            Some(c @ ('?' | '*' | '+')) => {
                self.pos += 1;
                self.out.push(c);
            }
            Some('{') => self.quantity()?,
            _ => {}
        }
        Ok(())
    }

    /// `quantity ::= '{' n (',' m?)? '}'`
    fn quantity(&mut self) -> Result<(), PatternError> {
        self.pos += 1; // '{'
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == '}' {
                break;
            }
            if !c.is_ascii_digit() && c != ',' {
                return Err(self.error("a quantifier takes digits and at most one comma"));
            }
            self.pos += 1;
        }
        if self.peek() != Some('}') {
            return Err(self.error("unterminated `{`"));
        }
        let body: String = self.chars[start..self.pos].iter().collect();
        self.pos += 1; // '}'
        if body.is_empty() || body.matches(',').count() > 1 {
            return Err(self.error("malformed quantifier"));
        }
        self.out.push('{');
        self.out.push_str(&body);
        self.out.push('}');
        Ok(())
    }

    fn atom(&mut self) -> Result<(), PatternError> {
        let Some(c) = self.next() else {
            return Err(self.error("unexpected end of pattern"));
        };
        match c {
            '(' => {
                // Non-capturing: XSD has no backreferences, so groups exist
                // only for precedence.
                self.out.push_str("(?:");
                self.regex()?;
                if self.next() != Some(')') {
                    return Err(self.error("unterminated `(`"));
                }
                self.out.push(')');
            }
            '[' => {
                let class = self.char_class()?;
                self.out.push_str(&class);
            }
            '.' => self.out.push('.'),
            '\\' => {
                let esc = self.escape()?;
                self.out.push_str(&esc);
            }
            // `^` and `$` are ordinary characters in XSD, which anchors the
            // whole pattern instead. Escaping them keeps that true.
            '^' | '$' => {
                self.out.push('\\');
                self.out.push(c);
            }
            ')' => return Err(self.error("unbalanced `)`")),
            other => {
                if regex_syntax::is_meta_character(other) {
                    self.out.push('\\');
                }
                self.out.push(other);
            }
        }
        Ok(())
    }

    /// A character class, resolving XSD's class subtraction.
    fn char_class(&mut self) -> Result<String, PatternError> {
        let mut negated = false;
        if self.peek() == Some('^') {
            self.pos += 1;
            negated = true;
        }
        let mut body = String::new();
        let mut subtraction: Option<String> = None;

        loop {
            match self.peek() {
                None => return Err(self.error("unterminated `[`")),
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                // `-[` opens a subtraction, but a `-` before `]` is a literal.
                Some('-') if self.chars.get(self.pos + 1) == Some(&'[') => {
                    self.pos += 2;
                    let inner = self.char_class_after_open()?;
                    subtraction = Some(inner);
                    if self.peek() != Some(']') {
                        return Err(self.error("a subtraction must close its outer `[`"));
                    }
                    self.pos += 1;
                    break;
                }
                Some('\\') => {
                    self.pos += 1;
                    body.push_str(&self.class_escape()?);
                }
                Some(c) => {
                    self.pos += 1;
                    if matches!(c, '[' | ']' | '^') {
                        body.push('\\');
                    }
                    body.push(c);
                }
            }
        }

        if body.is_empty() && subtraction.is_none() {
            return Err(self.error("an empty character class matches nothing"));
        }

        // `regex` spells subtraction `[a&&[^b]]`, which is exactly XSD's
        // `[a-[b]]`.
        Ok(match subtraction {
            Some(sub) => format!("[{}{}&&[^{}]]", if negated { "^" } else { "" }, body, sub),
            None => format!("[{}{}]", if negated { "^" } else { "" }, body),
        })
    }

    /// The inner class of a subtraction, whose `[` is already consumed.
    fn char_class_after_open(&mut self) -> Result<String, PatternError> {
        let full = self.char_class()?;
        // Strip the brackets the recursive call added; the caller re-wraps.
        Ok(full
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(&full)
            .to_string())
    }

    /// An escape outside a character class.
    fn escape(&mut self) -> Result<String, PatternError> {
        let Some(c) = self.next() else {
            return Err(self.error("pattern ends with `\\`"));
        };
        Ok(match c {
            'n' => "\\n".into(),
            'r' => "\\r".into(),
            't' => "\\t".into(),
            '\\' | '|' | '.' | '-' | '^' | '?' | '*' | '+' | '{' | '}' | '(' | ')' | '[' | ']' => {
                format!("\\{c}")
            }
            'd' | 'D' | 'w' | 'W' | 's' | 'S' => single_char_class(c),
            // XML name characters, which have no PCRE equivalent at all.
            'i' => format!("[{NAME_START}]"),
            'I' => format!("[^{NAME_START}]"),
            'c' => format!("[{NAME_CHAR}]"),
            'C' => format!("[^{NAME_CHAR}]"),
            'p' | 'P' => self.unicode_property(c == 'P')?,
            other => return Err(self.error(&format!("unknown escape `\\{other}`"))),
        })
    }

    /// An escape inside a character class, where the result must be class
    /// *contents* rather than a standalone class.
    fn class_escape(&mut self) -> Result<String, PatternError> {
        let Some(c) = self.next() else {
            return Err(self.error("class ends with `\\`"));
        };
        Ok(match c {
            'n' => "\\n".into(),
            'r' => "\\r".into(),
            't' => "\\t".into(),
            '\\' | '|' | '.' | '-' | '^' | '?' | '*' | '+' | '{' | '}' | '(' | ')' | '[' | ']' => {
                format!("\\{c}")
            }
            'd' => "0-9".into(),
            'D' => "^0-9".into(),
            'w' => "\\w".into(),
            'W' => "\\W".into(),
            's' => " \\t\\n\\r".into(),
            'S' => "^ \\t\\n\\r".into(),
            'i' => NAME_START.into(),
            'I' => format!("^{NAME_START}"),
            'c' => NAME_CHAR.into(),
            'C' => format!("^{NAME_CHAR}"),
            'p' | 'P' => {
                let cls = self.unicode_property(c == 'P')?;
                // Unwrap a bracketed block range for use inside a class.
                cls.strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(&cls)
                    .to_string()
            }
            other => return Err(self.error(&format!("unknown class escape `\\{other}`"))),
        })
    }

    /// `\p{...}` — either a Unicode general category, which `regex` shares,
    /// or an XSD Unicode *block* (`IsBasicLatin`), which it does not.
    fn unicode_property(&mut self, negated: bool) -> Result<String, PatternError> {
        if self.next() != Some('{') {
            return Err(self.error("`\\p` must be followed by `{`"));
        }
        let start = self.pos;
        while self.peek().is_some_and(|c| c != '}') {
            self.pos += 1;
        }
        if self.peek() != Some('}') {
            return Err(self.error("unterminated `\\p{`"));
        }
        let name: String = self.chars[start..self.pos].iter().collect();
        self.pos += 1;

        if let Some(block) = name.strip_prefix("Is") {
            let (lo, hi) = unicode_block(block)
                .ok_or_else(|| self.error(&format!("unknown Unicode block `Is{block}`")))?;
            return Ok(format!(
                "[{}\\u{{{lo:04X}}}-\\u{{{hi:04X}}}]",
                if negated { "^" } else { "" }
            ));
        }
        Ok(format!("\\{}{{{}}}", if negated { 'P' } else { 'p' }, name))
    }
}

fn single_char_class(c: char) -> String {
    match c {
        'd' => "[0-9]".into(),
        'D' => "[^0-9]".into(),
        'w' => "\\w".into(),
        'W' => "\\W".into(),
        // XSD's \s is exactly these four, not Unicode whitespace.
        's' => "[ \\t\\n\\r]".into(),
        'S' => "[^ \\t\\n\\r]".into(),
        _ => unreachable!(),
    }
}

/// XML `NameStartChar`, as character-class contents.
const NAME_START: &str = ":A-Z_a-z\\u{C0}-\\u{D6}\\u{D8}-\\u{F6}\\u{F8}-\\u{2FF}\
\\u{370}-\\u{37D}\\u{37F}-\\u{1FFF}\\u{200C}-\\u{200D}\\u{2070}-\\u{218F}\
\\u{2C00}-\\u{2FEF}\\u{3001}-\\u{D7FF}\\u{F900}-\\u{FDCF}\\u{FDF0}-\\u{FFFD}\
\\u{10000}-\\u{EFFFF}";

/// XML `NameChar`, as character-class contents.
const NAME_CHAR: &str = ":A-Z_a-z\\u{C0}-\\u{D6}\\u{D8}-\\u{F6}\\u{F8}-\\u{2FF}\
\\u{370}-\\u{37D}\\u{37F}-\\u{1FFF}\\u{200C}-\\u{200D}\\u{2070}-\\u{218F}\
\\u{2C00}-\\u{2FEF}\\u{3001}-\\u{D7FF}\\u{F900}-\\u{FDCF}\\u{FDF0}-\\u{FFFD}\
\\u{10000}-\\u{EFFFF}\\-.0-9\\u{B7}\\u{300}-\\u{36F}\\u{203F}-\\u{2040}";

/// The Unicode blocks XSD names with `\p{IsXxx}`.
///
/// Only the blocks that appear in real schemas; an unknown one is a
/// diagnostic rather than a silent mismatch.
fn unicode_block(name: &str) -> Option<(u32, u32)> {
    Some(match name {
        "BasicLatin" => (0x0000, 0x007F),
        "Latin-1Supplement" => (0x0080, 0x00FF),
        "LatinExtended-A" => (0x0100, 0x017F),
        "LatinExtended-B" => (0x0180, 0x024F),
        "IPAExtensions" => (0x0250, 0x02AF),
        "SpacingModifierLetters" => (0x02B0, 0x02FF),
        "CombiningDiacriticalMarks" => (0x0300, 0x036F),
        "Greek" | "GreekandCoptic" => (0x0370, 0x03FF),
        "Cyrillic" => (0x0400, 0x04FF),
        "Hebrew" => (0x0590, 0x05FF),
        "Arabic" => (0x0600, 0x06FF),
        "Thai" => (0x0E00, 0x0E7F),
        "GeneralPunctuation" => (0x2000, 0x206F),
        "SuperscriptsandSubscripts" => (0x2070, 0x209F),
        "CurrencySymbols" => (0x20A0, 0x20CF),
        "LetterlikeSymbols" => (0x2100, 0x214F),
        "NumberForms" => (0x2150, 0x218F),
        "Arrows" => (0x2190, 0x21FF),
        "MathematicalOperators" => (0x2200, 0x22FF),
        "BoxDrawing" => (0x2500, 0x257F),
        "GeometricShapes" => (0x25A0, 0x25FF),
        "MiscellaneousSymbols" => (0x2600, 0x26FF),
        "Hiragana" => (0x3040, 0x309F),
        "Katakana" => (0x30A0, 0x30FF),
        "CJKUnifiedIdeographs" => (0x4E00, 0x9FFF),
        "HangulSyllables" => (0xAC00, 0xD7AF),
        "Specials" => (0xFFF0, 0xFFFF),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, value: &str) -> bool {
        PatternStep::compile(&[pattern.to_string()])
            .unwrap_or_else(|e| panic!("{e}"))
            .is_match(value)
    }

    /// An XSD pattern matches the whole value, always.
    #[test]
    fn patterns_are_implicitly_anchored() {
        assert!(matches("[a-z]+", "abc"));
        assert!(!matches("[a-z]+", "abc1"), "a partial match is not a match");
        assert!(!matches("b", "abc"));
    }

    /// `^` and `$` are ordinary characters in XSD, not anchors — but they are
    /// not symmetrical. `SingleCharEsc` admits `\^` and does **not** admit
    /// `\$`, so escaping a dollar is an invalid pattern rather than a
    /// redundant one.
    #[test]
    fn caret_and_dollar_are_literals() {
        assert!(matches("\\^a", "^a"));
        assert!(matches("^a", "^a"), "an unescaped ^ is still a literal");
        assert!(!matches("^a", "a"));

        assert!(matches("a$", "a$"), "an unescaped $ is a literal");
        assert!(!matches("a$", "a"));
        assert!(
            PatternStep::compile(&["a\\$".into()]).is_err(),
            "`\\$` is not one of XSD's SingleCharEsc characters"
        );
    }

    #[test]
    fn alternatives_within_a_step_are_ored() {
        let step = PatternStep::compile(&["[A-Z]+".into(), "[0-9]+".into()]).unwrap();
        assert!(step.is_match("ABC"));
        assert!(step.is_match("123"));
        assert!(!step.is_match("A1"));
    }

    /// Patterns AND across restriction steps even though they OR within one.
    #[test]
    fn steps_are_anded() {
        let p = Patterns::compile(&[
            vec!["[A-Za-z]+".into(), "[0-9]+".into()],
            vec![".{3}".into()],
        ])
        .unwrap();
        assert!(p.is_match("abc"), "letters and exactly three characters");
        assert!(p.is_match("123"));
        assert!(!p.is_match("ab"), "fails the second step");
        assert!(!p.is_match("ab1"), "fails the first");
    }

    /// The headline difference from PCRE.
    #[test]
    fn character_class_subtraction() {
        assert!(matches("[a-z-[aeiou]]+", "bcdfg"));
        assert!(!matches("[a-z-[aeiou]]+", "abc"));
        assert!(matches("[a-z-[aeiou]]", "z"));
    }

    #[test]
    fn xml_name_escapes() {
        assert!(matches("\\i\\c*", "well_1"));
        assert!(matches("\\i\\c*", "ns:well"));
        assert!(
            !matches("\\i\\c*", "1well"),
            "a name cannot start with a digit"
        );
        assert!(matches("\\c+", "1well"), "\\c admits digits");
        assert!(!matches("\\i", "-"));
    }

    #[test]
    fn unicode_blocks_become_ranges() {
        assert!(matches("\\p{IsBasicLatin}+", "hello"));
        assert!(!matches("\\p{IsBasicLatin}+", "héllo"));
        assert!(matches("\\p{IsGreek}+", "αβγ"));
        assert!(matches("\\P{IsBasicLatin}+", "αβγ"));
    }

    #[test]
    fn unicode_categories_pass_through() {
        assert!(matches("\\p{Lu}+", "ABC"));
        assert!(!matches("\\p{Lu}+", "abc"));
        assert!(matches("\\p{Nd}+", "123"));
    }

    /// XSD's `\s` is exactly space, tab, newline and carriage return — not
    /// Unicode whitespace.
    #[test]
    fn whitespace_escape_is_the_xsd_set() {
        assert!(matches("\\s+", " \t\n\r"));
        assert!(!matches("\\s", "\u{A0}"), "no-break space is not XSD \\s");
        assert!(matches("\\S+", "abc"));
    }

    #[test]
    fn quantifiers_and_groups() {
        assert!(matches("(ab)+", "ababab"));
        assert!(matches("a{2,3}", "aa"));
        assert!(matches("a{2,3}", "aaa"));
        assert!(!matches("a{2,3}", "a"));
        assert!(!matches("a{2,3}", "aaaa"));
        assert!(matches("a{3}", "aaa"));
        assert!(matches("(a|b)c", "bc"));
    }

    #[test]
    fn metacharacters_are_escaped_not_interpreted() {
        // A literal `#` and `%` must survive translation untouched.
        assert!(matches("[0-9]{4}#[0-9]{2}", "2024#12"));
        assert!(matches("a\\.b", "a.b"));
        assert!(!matches("a\\.b", "axb"));
    }

    #[test]
    fn real_world_patterns() {
        // A UUID, as many schemas spell it.
        let uuid = "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}";
        assert!(matches(uuid, "123e4567-e89b-12d3-a456-426614174000"));
        assert!(!matches(uuid, "123e4567-e89b-12d3-a456"));

        // An ISO currency code.
        assert!(matches("[A-Z]{3}", "EUR"));
        // A WITSML-style unit symbol.
        assert!(matches("[^\\s]+", "kg/m3"));
    }

    #[test]
    fn malformed_patterns_are_reported_not_panicked() {
        for bad in [
            "[a-z",
            "(abc",
            "abc)",
            "a{",
            "a{1,2,3}",
            "\\q",
            "[]",
            "\\p{Nope}",
            "\\",
        ] {
            let e = PatternStep::compile(&[bad.to_string()]);
            assert!(e.is_err(), "`{bad}` should not compile");
        }
    }

    #[test]
    fn an_unknown_block_is_named_in_the_error() {
        let e = PatternStep::compile(&["\\p{IsKlingon}".into()]).unwrap_err();
        assert!(e.to_string().contains("IsKlingon"), "{e}");
    }
}
