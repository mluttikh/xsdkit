//! Turning schema document bytes into text.
//!
//! `roxmltree` parses `&str`, so every document has to be decoded before it
//! is parsed. That decoding happens **here and only here** — the [`Resolver`]
//! trait hands back bytes precisely so no resolver has to reimplement this
//! and get it wrong in its own way.
//!
//! The order follows XML 1.0 Appendix F:
//!
//! 1. A byte-order mark, if present, decides the encoding outright.
//! 2. Otherwise the `encoding` pseudo-attribute of the XML declaration.
//! 3. Otherwise UTF-8.
//!
//! [`Resolver`]: crate::load::Resolver

use crate::diagnostics::{DiagCode, Diagnostic, Span};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

/// How the encoding of a document was determined.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum EncodingSource {
    /// A byte-order mark.
    Bom,
    /// The `encoding=` pseudo-attribute of the XML declaration.
    Declaration,
    /// Nothing said; XML's default applies.
    Default,
}

/// What a decode produced, alongside how the encoding was chosen.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub text: String,
    pub encoding: &'static str,
    pub source: EncodingSource,
}

/// Decodes a schema document.
///
/// Returns a diagnostic rather than a string on failure, so an encoding
/// problem reads as an encoding problem — it used to surface as
/// [`DiagCode::UnresolvedSchemaLocation`], blaming a file that had been found
/// and read perfectly well.
pub fn decode_document(bytes: &[u8], uri: &str) -> Result<Decoded, Diagnostic> {
    let span = || Span::new(uri, 0);

    // 1. A BOM is decisive, and is not part of the document.
    if let Some((encoding, rest)) = strip_bom(bytes) {
        return finish(encoding, rest, EncodingSource::Bom, uri);
    }

    // 2. The declaration. XML requires a BOM for UTF-16, so anything reaching
    //    here is ASCII-compatible in its first bytes and can be scanned raw.
    if let Some(label) = declared_encoding(bytes) {
        let Some(encoding) = Encoding::for_label(label.as_bytes()) else {
            return Err(Diagnostic::error(
                DiagCode::UnsupportedEncoding,
                format!("unknown encoding `{label}`"),
            )
            .at(span())
            .with_help("use an encoding label from the WHATWG Encoding Standard, such as UTF-8"));
        };
        return finish(encoding, bytes, EncodingSource::Declaration, uri);
    }

    // 3. XML's default.
    finish(UTF_8, bytes, EncodingSource::Default, uri)
}

fn finish(
    encoding: &'static Encoding,
    bytes: &[u8],
    source: EncodingSource,
    uri: &str,
) -> Result<Decoded, Diagnostic> {
    // Without replacement: a byte sequence that is not valid in the encoding
    // it claims is an error, not a document full of U+FFFD.
    let Some(text) = encoding.decode_without_bom_handling_and_without_replacement(bytes) else {
        return Err(Diagnostic::error(
            DiagCode::MalformedEncoding,
            format!("bytes are not valid {}", encoding.name()),
        )
        .at(Span::new(uri, 0))
        .with_help(match source {
            EncodingSource::Declaration => "the XML declaration may name the wrong encoding",
            EncodingSource::Bom => "the byte-order mark disagrees with the document's contents",
            EncodingSource::Default => "no encoding was declared, so UTF-8 was assumed",
        }));
    };
    Ok(Decoded {
        text: text.into_owned(),
        encoding: encoding.name(),
        source,
    })
}

/// Splits off a byte-order mark, returning the encoding it names.
fn strip_bom(bytes: &[u8]) -> Option<(&'static Encoding, &[u8])> {
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => Some((UTF_8, rest)),
        // UTF-32 BOMs start with the same two bytes as UTF-16, so they must be
        // tested first to avoid being mistaken for one.
        [0xFF, 0xFE, 0x00, 0x00, ..] | [0x00, 0x00, 0xFE, 0xFF, ..] => None,
        [0xFF, 0xFE, rest @ ..] => Some((UTF_16LE, rest)),
        [0xFE, 0xFF, rest @ ..] => Some((UTF_16BE, rest)),
        _ => None,
    }
}

/// Reads the `encoding` pseudo-attribute out of an XML declaration.
///
/// Scans only the declaration itself: anything past `?>` is document content,
/// where the word `encoding` carries no such meaning.
fn declared_encoding(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let decl_end = text.find("?>")?;
    let decl = &text[..decl_end];
    if !decl.trim_start().starts_with("<?xml") {
        return None;
    }
    let at = decl.find("encoding")?;
    let rest = decl[at + "encoding".len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[1..];
    let end = value.find(quote)?;
    Some(value[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latin1(s: &str) -> Vec<u8> {
        encoding_rs::WINDOWS_1252.encode(s).0.into_owned()
    }

    #[test]
    fn plain_utf8_needs_no_declaration() {
        let d = decode_document(b"<a>hi</a>", "u").unwrap();
        assert_eq!(d.text, "<a>hi</a>");
        assert_eq!(d.source, EncodingSource::Default);
        assert_eq!(d.encoding, "UTF-8");
    }

    #[test]
    fn a_utf8_bom_is_stripped() {
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(b"<a/>");
        let d = decode_document(&b, "u").unwrap();
        assert_eq!(d.text, "<a/>", "the BOM must not survive into the document");
        assert_eq!(d.source, EncodingSource::Bom);
    }

    /// encoding_rs has no UTF-16 *encoder* — per WHATWG, `encode` falls back
    /// to UTF-8 — so these bytes are built by hand.
    fn utf16(s: &str, little_endian: bool) -> Vec<u8> {
        let mut out = if little_endian {
            vec![0xFF, 0xFE]
        } else {
            vec![0xFE, 0xFF]
        };
        for unit in s.encode_utf16() {
            out.extend_from_slice(&if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        out
    }

    #[test]
    fn utf16_is_recognised_from_its_bom() {
        for little_endian in [true, false] {
            let b = utf16("<a>é 日本</a>", little_endian);
            let d = decode_document(&b, "u").unwrap();
            assert_eq!(d.text, "<a>é 日本</a>", "little_endian={little_endian}");
            assert_eq!(d.source, EncodingSource::Bom);
            assert!(d.encoding.starts_with("UTF-16"), "{}", d.encoding);
        }
    }

    /// A surrogate pair must survive, which a naive two-bytes-per-char
    /// decoder would mangle.
    #[test]
    fn utf16_handles_astral_characters() {
        let d = decode_document(&utf16("<a>🜁</a>", true), "u").unwrap();
        assert_eq!(d.text, "<a>🜁</a>");
    }

    #[test]
    fn a_declared_encoding_is_honoured() {
        let doc = r#"<?xml version="1.0" encoding="ISO-8859-1"?><a>Größe</a>"#;
        let d = decode_document(&latin1(doc), "u").unwrap();
        assert!(d.text.contains("Größe"), "{}", d.text);
        assert_eq!(d.source, EncodingSource::Declaration);
        assert_eq!(
            d.encoding, "windows-1252",
            "ISO-8859-1 maps to windows-1252 per WHATWG"
        );
    }

    #[test]
    fn single_quotes_and_spacing_are_accepted() {
        let doc = "<?xml version='1.0'  encoding = 'ISO-8859-1' ?><a>ä</a>";
        let d = decode_document(&latin1(doc), "u").unwrap();
        assert!(d.text.contains('ä'));
    }

    #[test]
    fn an_unknown_encoding_is_named_as_such() {
        let doc = br#"<?xml version="1.0" encoding="KOI9-Klingon"?><a/>"#;
        let e = decode_document(doc, "u").unwrap_err();
        assert_eq!(e.code, DiagCode::UnsupportedEncoding);
        assert!(e.message.contains("KOI9-Klingon"), "{}", e.message);
    }

    #[test]
    fn bytes_that_contradict_their_declaration_are_an_error() {
        // Declares UTF-8, contains a lone 0xE9 — valid Latin-1, invalid UTF-8.
        let mut b = br#"<?xml version="1.0" encoding="UTF-8"?><a>"#.to_vec();
        b.push(0xE9);
        b.extend_from_slice(b"</a>");
        let e = decode_document(&b, "u").unwrap_err();
        assert_eq!(e.code, DiagCode::MalformedEncoding);
        assert!(e.help.as_ref().unwrap().contains("wrong encoding"));
    }

    #[test]
    fn undeclared_non_utf8_bytes_are_an_error_not_replacement_characters() {
        let e =
            decode_document(&[b'<', b'a', b'>', 0xFF, b'<', b'/', b'a', b'>'], "u").unwrap_err();
        assert_eq!(e.code, DiagCode::MalformedEncoding);
        assert!(
            !e.message.contains('\u{FFFD}'),
            "must not silently substitute"
        );
    }

    /// The word `encoding` after the declaration is ordinary content.
    #[test]
    fn encoding_is_only_read_from_the_declaration() {
        let doc = br#"<?xml version="1.0"?><a encoding="ISO-8859-1"/>"#;
        let d = decode_document(doc, "u").unwrap();
        assert_eq!(d.source, EncodingSource::Default);
    }

    #[test]
    fn a_document_with_no_declaration_at_all_is_utf8() {
        let d = decode_document("<a>日本語</a>".as_bytes(), "u").unwrap();
        assert!(d.text.contains("日本語"));
    }

    #[test]
    fn a_utf32_bom_is_not_mistaken_for_utf16() {
        // UTF-32LE starts FF FE 00 00, which is a UTF-16LE BOM plus a NUL.
        let b = [0xFF, 0xFE, 0x00, 0x00, 0x3C, 0x00, 0x00, 0x00];
        // Not claimed as UTF-16; falls through and fails honestly.
        assert!(decode_document(&b, "u").is_err());
    }
}
