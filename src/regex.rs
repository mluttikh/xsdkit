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

mod blocks;
pub(crate) use blocks::UNICODE_VERSION;

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

/// Why one restriction step's patterns could not be compiled, told apart by
/// whose limitation it is.
#[derive(Clone, Debug)]
pub(crate) enum StepError {
    /// Not an XSD regular expression at all, so the schema is in error.
    Invalid(PatternError),
    /// A valid XSD pattern that the `regex` crate will not compile — most
    /// often one past its size limit. The schema is fine; this crate cannot
    /// enforce the step.
    Unsupported(PatternError),
}

impl StepError {
    pub(crate) fn into_error(self) -> PatternError {
        match self {
            StepError::Invalid(e) | StepError::Unsupported(e) => e,
        }
    }
}

impl PatternStep {
    /// Compiles the alternatives declared at one restriction step.
    pub fn compile(alternatives: &[String]) -> Result<Self, PatternError> {
        Self::compile_noting(alternatives)
            .map(|(step, _)| step)
            .map_err(StepError::into_error)
    }

    /// [`Self::compile`], saying why a step failed and which block names it
    /// used that this build does not recognise.
    ///
    /// An unrecognised block is not a failure: XSD 1.1 §G.4.2.4 says the
    /// escape matches every character, and that a processor should warn.
    pub(crate) fn compile_noting(
        alternatives: &[String],
    ) -> Result<(Self, Vec<String>), StepError> {
        let mut branches = Vec::with_capacity(alternatives.len());
        let mut unknown_blocks = Vec::new();
        for a in alternatives {
            let (translated, unknown) = translate_noting(a).map_err(StepError::Invalid)?;
            branches.push(format!("(?:{translated})"));
            unknown_blocks.extend(unknown);
        }
        // Implicitly anchored: an XSD pattern matches the *whole* value.
        let joined = format!("^(?:{})$", branches.join("|"));
        Regex::new(&joined)
            .map(|r| (PatternStep(r), unknown_blocks))
            .map_err(|e| {
                // The engine's own report quotes the translated expression,
                // which is not what the schema wrote; its last line is the
                // reason.
                let full = e.to_string();
                let reason = full
                    .lines()
                    .rev()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or(&full)
                    .trim_start_matches("error: ")
                    .to_string();
                StepError::Unsupported(PatternError {
                    pattern: alternatives.join("|"),
                    reason,
                })
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
    translate_noting(pattern).map(|(translated, _)| translated)
}

/// [`translate`], with the block names used that this build does not know.
fn translate_noting(pattern: &str) -> Result<(String, Vec<String>), PatternError> {
    let mut t = Translator {
        chars: pattern.chars().collect(),
        pos: 0,
        out: String::with_capacity(pattern.len() + 8),
        source: pattern,
        unknown_blocks: Vec::new(),
    };
    t.regex()?;
    if t.pos != t.chars.len() {
        return Err(t.error("unbalanced `)`"));
    }
    Ok((t.out, t.unknown_blocks))
}

struct Translator<'a> {
    chars: Vec<char>,
    pos: usize,
    out: String,
    source: &'a str,
    /// `\p{IsX}` names that are not blocks this build knows, in order.
    unknown_blocks: Vec<String>,
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
            // A category is `\p{Lu}`, which the `regex` crate reads inside a
            // class as it does outside one. A block is a bracketed class of
            // its own, and a class may nest another: splicing its contents in
            // instead turned `\P{IsX}` into a literal `^` beside the block.
            'p' | 'P' => self.unicode_property(c == 'P')?,
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
            // `IsBlock ::= 'Is' [a-zA-Z0-9#x2D]+`. Anything else is not a
            // block escape, and so not a regular expression.
            if block.is_empty() || !block.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(self.error(&format!("`Is{block}` is not a block name")));
            }
            return Ok(self.block_class(block, negated));
        }
        // The categories are a fixed grammar, not whatever the `regex` crate
        // happens to accept: `\p{Greek}` is a script there and nothing here,
        // and `\p{Cs}` is excluded because no XML character is a surrogate.
        if !is_xsd_category(&name) {
            return Err(self.error(&format!(
                "`{name}` is neither a Unicode category nor an `Is` block name"
            )));
        }
        Ok(format!("\\{}{{{}}}", if negated { 'P' } else { 'p' }, name))
    }

    /// `\p{IsX}` or `\P{IsX}`, as a bracketed class that reads the same on its
    /// own and nested inside another class.
    ///
    /// Two sets need spelling out, since the `regex` crate has no syntax for
    /// either: every character, and none.
    fn block_class(&mut self, name: &str, negated: bool) -> String {
        const EVERY: &str = "[\\u{0}-\\u{10FFFF}]";
        const NONE: &str = "[^\\u{0}-\\u{10FFFF}]";
        let Ok(at) = blocks::BLOCKS.binary_search_by(|(n, _)| (*n).cmp(name)) else {
            // XSD 1.1 §G.4.2.4: an unrecognised block name is not an error by
            // default. `\p{IsX}` and `\P{IsX}` both denote every character, so
            // the constraint goes unenforced, and the processor should warn.
            self.unknown_blocks.push(name.to_string());
            return EVERY.to_string();
        };
        let ranges: String = scalar_ranges(blocks::BLOCKS[at].1)
            .map(|(lo, hi)| format!("\\u{{{lo:X}}}-\\u{{{hi:X}}}"))
            .collect();
        // The surrogate blocks hold no character a document can contain, so
        // the block is empty and its complement is everything.
        match (ranges.is_empty(), negated) {
            (true, false) => NONE.to_string(),
            (true, true) => EVERY.to_string(),
            (false, false) => format!("[{ranges}]"),
            (false, true) => format!("[^{ranges}]"),
        }
    }
}

/// A block's ranges with the surrogates cut out, which the `regex` crate
/// refuses to name because they are not Unicode scalar values.
fn scalar_ranges(ranges: &[(u32, u32)]) -> impl Iterator<Item = (u32, u32)> + '_ {
    ranges.iter().flat_map(|&(lo, hi)| {
        [(lo, hi.min(0xD7FF)), (lo.max(0xE000), hi)]
            .into_iter()
            .filter(|(a, b)| a <= b)
    })
}

/// `IsCategory` in XSD: a general category or one of its groups. `Cs` is
/// absent on purpose.
fn is_xsd_category(name: &str) -> bool {
    let mut chars = name.chars();
    let (Some(group), rest) = (chars.next(), chars.as_str()) else {
        return false;
    };
    let members = match group {
        'L' => "ultmo",
        'M' => "nce",
        'N' => "dlo",
        'P' => "cdseifo",
        'Z' => "slp",
        'S' => "mcko",
        'C' => "cfon",
        _ => return false,
    };
    rest.is_empty() || (rest.len() == 1 && members.contains(rest))
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

    /// XSD 1.1 §G.4.2.4: a name that fits `IsBlock` but names no block this
    /// processor knows is not an error. `\p{IsX}` and `\P{IsX}` both denote
    /// every character, and the name is handed back so a warning can say so.
    #[test]
    fn an_unknown_block_matches_every_character_and_is_named() {
        for escape in ["\\p{IsKlingon}", "\\P{IsKlingon}", "[\\p{IsKlingon}]"] {
            let (step, unknown) = PatternStep::compile_noting(&[format!("{escape}+")])
                .unwrap_or_else(|e| panic!("{escape}: {:?}", e));
            assert_eq!(unknown, vec!["Klingon".to_string()], "{escape}");
            for value in ["a", "é", "日本", "\n\r"] {
                assert!(step.is_match(value), "{escape} on {value:?}");
            }
        }
    }

    /// Every block Unicode defines, not the few that used to be listed — a
    /// name outside the old table failed to translate, and the whole pattern
    /// was then dropped without a word.
    #[test]
    fn every_unicode_block_is_recognised() {
        assert!(matches("\\p{IsCJKCompatibility}", "\u{3300}"));
        assert!(!matches("\\p{IsCJKCompatibility}", "a"));
        assert!(matches("\\p{IsLatin-1Supplement}+", "éü"));
        assert!(matches(
            "\\p{IsMathematicalAlphanumericSymbols}",
            "\u{1D400}"
        ));
        // Hyphens and case are part of the name; nothing is normalised.
        let (_, unknown) = PatternStep::compile_noting(&["\\p{Islatin-1supplement}".into()])
            .expect("an unknown name still compiles");
        assert_eq!(unknown, vec!["latin-1supplement".to_string()]);
        assert_eq!(blocks::UNICODE_VERSION, "16.0.0");
        assert!(
            blocks::BLOCKS.windows(2).all(|w| w[0].0 < w[1].0),
            "the table must be sorted and unique for its binary search"
        );
    }

    /// XSD 1.1 asks for the Unicode 3.1 names that XSD 1.0 schemas use and
    /// later versions of Unicode renamed.
    #[test]
    fn the_superseded_unicode_3_1_names_still_work() {
        assert!(matches("\\p{IsGreek}+", "αβγ"));
        assert!(matches("\\p{IsCombiningMarksforSymbols}", "\u{20D0}"));
        for c in ["\u{E000}", "\u{F0000}", "\u{10FFFD}"] {
            assert!(matches("\\p{IsPrivateUse}", c), "{c:?}");
        }
    }

    /// No XML character is a surrogate, so the surrogate blocks are empty —
    /// and the `regex` crate refuses to name those code points at all.
    #[test]
    fn a_surrogate_block_is_empty() {
        assert!(!matches("\\p{IsHighSurrogates}", "a"));
        assert!(matches("\\P{IsHighSurrogates}", "a"));
        assert!(matches("[a\\p{IsLowSurrogates}]", "a"));
    }

    /// A block inside a class is a class of its own. Its contents used to be
    /// spliced in, which made `\P{IsX}` a literal `^` beside the block.
    #[test]
    fn a_negated_block_inside_a_class() {
        assert!(matches("[a\\P{IsBasicLatin}]", "a"));
        assert!(matches("[a\\P{IsBasicLatin}]", "é"));
        assert!(!matches("[a\\P{IsBasicLatin}]", "b"));
        assert!(!matches("[a\\P{IsBasicLatin}]", "^"));
        assert!(matches("[\\p{IsBasicLatin}-[a-z]]", "Q"));
        assert!(!matches("[\\p{IsBasicLatin}-[a-z]]", "q"));
    }

    /// The categories are XSD's grammar, not whatever the `regex` crate
    /// accepts: it knows scripts and binary properties that XSD does not, and
    /// XSD leaves out `Cs`.
    #[test]
    fn only_xsd_categories_are_categories() {
        for ok in ["L", "Lu", "Nd", "P", "Pi", "Zs", "So", "C", "Cn"] {
            assert!(
                PatternStep::compile(&[format!("\\p{{{ok}}}")]).is_ok(),
                "\\p{{{ok}}}"
            );
        }
        for bad in [
            "Greek",
            "Alphabetic",
            "Cs",
            "Lx",
            "Lul",
            "",
            "Is",
            "Is Greek",
        ] {
            let e = PatternStep::compile_noting(&[format!("\\p{{{bad}}}")]);
            assert!(
                matches!(e, Err(StepError::Invalid(_))),
                "\\p{{{bad}}} must be an invalid pattern, got {e:?}"
            );
        }
    }

    /// A valid pattern the engine refuses is told apart from an invalid one:
    /// the first is this crate's limit, the second the schema's mistake.
    #[test]
    fn the_engine_refusing_a_pattern_is_not_the_pattern_being_invalid() {
        let e = PatternStep::compile_noting(&["(a{1000}){1000}".into()]);
        let Err(StepError::Unsupported(e)) = e else {
            panic!("expected the engine to refuse it, got {e:?}");
        };
        assert!(e.reason.contains("size limit"), "{}", e.reason);
        assert!(
            !e.reason.contains('\n'),
            "the reason is one line, not the engine's quoted expression: {}",
            e.reason
        );
        assert!(matches!(
            PatternStep::compile_noting(&["[^]".into()]),
            Err(StepError::Invalid(_))
        ));
    }
}
