//! COMPOUND_TEXT conversion for GPUI's XIM client.
//! The unescaped encoding is Latin-1, not necessarily UTF-8.

#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::{string::String, vec::Vec};
use core::fmt;

#[cfg(feature = "std")]
use std::io::{self, Write};

const UTF8_START: &[u8] = b"\x1b%G";
const UTF8_END: &[u8] = b"\x1b%@";

#[derive(Clone, Copy)]
pub struct CText<'a> {
    text: &'a str,
}

impl<'a> CText<'a> {
    pub const fn new(text: &'a str) -> Self {
        Self { text }
    }

    pub const fn len(self) -> usize {
        self.text.len() + UTF8_START.len() + UTF8_END.len()
    }

    #[cfg(feature = "std")]
    pub fn write(self, mut out: impl Write) -> io::Result<usize> {
        out.write_all(UTF8_START)?;
        out.write_all(self.text.as_bytes())?;
        out.write_all(UTF8_END)?;
        Ok(self.len())
    }
}

impl fmt::Display for CText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}

impl fmt::Debug for CText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}

#[derive(Debug)]
pub enum DecodeError {
    InvalidEncoding,
    UnsupportedEncoding,
    Utf8Error(alloc::string::FromUtf8Error),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub fn utf8_to_compound_text(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 6);
    bytes.extend_from_slice(UTF8_START);
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(UTF8_END);
    bytes
}

pub fn compound_text_to_utf8(bytes: &[u8]) -> Result<String, DecodeError> {
    if bytes.starts_with(UTF8_START) {
        let contents = bytes[UTF8_START.len()..]
            .strip_suffix(UTF8_END)
            .ok_or(DecodeError::InvalidEncoding)?;
        return String::from_utf8(contents.to_vec()).map_err(DecodeError::Utf8Error);
    }
    if bytes.starts_with(b"\x1b$(B") {
        // COMPOUND_TEXT's JP designation differs from ISO-2022-JP's.
        let mut iso_2022_jp = b"\x1b$B".to_vec();
        iso_2022_jp.extend_from_slice(&bytes[4..]);
        let (text, _, had_errors) = encoding_rs::ISO_2022_JP.decode(&iso_2022_jp);
        return if had_errors {
            Err(DecodeError::InvalidEncoding)
        } else {
            Ok(text.into_owned())
        };
    }
    if bytes.starts_with(b"\x1b$(A") || bytes.starts_with(b"\x1b$(C") {
        return Err(DecodeError::UnsupportedEncoding);
    }
    if bytes.first() == Some(&0x1b) {
        return Err(DecodeError::InvalidEncoding);
    }

    // XIM servers may send the default COMPOUND_TEXT charset (ISO-8859-1)
    // without a designator. Its byte 0xb4 is the acute accent dead key.
    match String::from_utf8(bytes.to_vec()) {
        Ok(text) => Ok(text),
        Err(_) => Ok(bytes.iter().map(|&byte| char::from(byte)).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_unescaped_latin1_accent_and_utf8_escapes() {
        assert_eq!(compound_text_to_utf8(&[0xb4]).unwrap(), "´");
        assert_eq!(compound_text_to_utf8(&[0xe1]).unwrap(), "á");
        assert_eq!(
            compound_text_to_utf8(&utf8_to_compound_text("á€")).unwrap(),
            "á€"
        );
        assert_eq!(compound_text_to_utf8("á".as_bytes()).unwrap(), "á");
    }

    #[test]
    fn malformed_escape_is_an_error_instead_of_a_panic() {
        assert!(compound_text_to_utf8(b"\x1b%G").is_err());
    }

    #[test]
    fn retains_japanese_compound_text_support() {
        assert_eq!(
            compound_text_to_utf8(&[0x1b, 0x24, 0x28, 0x42, 0x45, 0x6c, 0x35, 0x7e]).unwrap(),
            "東京"
        );
    }
}
