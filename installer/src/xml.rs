// SPDX-License-Identifier: MIT
//! The small part of `xml.etree.ElementTree` the installer edits settings with.
//!
//! A document is read into elements with `text` and `tail` exactly as
//! ElementTree does, changed, and written back with ElementTree's quoting, so
//! untouched settings keep their bytes. Unlike ElementTree, comments and
//! processing instructions inside the root element are kept.
use crate::error::{Error, Result};
use quick_xml::{Reader, XmlVersion, escape::resolve_predefined_entity, events::Event};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Element {
    pub tag: String,
    pub attributes: Vec<(String, String)>,
    /// Character data before the first child.
    pub text: String,
    pub children: Vec<Node>,
    /// Character data after this element's end tag.
    pub tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Comment { text: String, tail: String },
    Instruction { text: String, tail: String },
}

impl Node {
    fn tail_mut(&mut self) -> &mut String {
        match self {
            Node::Element(element) => &mut element.tail,
            Node::Comment { tail, .. } | Node::Instruction { tail, .. } => tail,
        }
    }
}

const MAX_DEPTH: usize = 256;

impl Element {
    pub fn new(tag: &str) -> Self {
        Self {
            tag: tag.to_string(),
            ..Self::default()
        }
    }

    /// `ET.SubElement(self, tag)` with its text set.
    pub fn append(&mut self, tag: &str, text: &str) -> &mut Element {
        let mut child = Element::new(tag);
        child.text = text.to_string();
        self.children.push(Node::Element(child));
        match self.children.last_mut() {
            Some(Node::Element(child)) => child,
            _ => unreachable!("an element was just appended"),
        }
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|node| match node {
            Node::Element(element) => Some(element),
            _ => None,
        })
    }

    /// `self.find(tag)`: the first direct child with this tag.
    pub fn find(&self, tag: &str) -> Option<&Element> {
        self.elements().find(|element| element.tag == tag)
    }

    /// `self.findtext(tag, "")`.
    pub fn findtext(&self, tag: &str) -> &str {
        self.find(tag).map_or("", |element| &element.text)
    }

    /// Set the text of the first child with this tag, appending one if there is none.
    pub fn set(&mut self, tag: &str, value: &str) {
        let found = self.children.iter_mut().find_map(|node| match node {
            Node::Element(element) if element.tag == tag => Some(element),
            _ => None,
        });
        match found {
            Some(element) => element.text = value.to_string(),
            None => {
                self.append(tag, value);
            }
        }
    }

    /// `ET.tostring(self, encoding="utf-8", xml_declaration=True)`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::from("<?xml version='1.0' encoding='utf-8'?>\n");
        self.write(&mut out);
        out.into_bytes()
    }

    fn write(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.tag);
        for (key, value) in &self.attributes {
            out.push(' ');
            out.push_str(key);
            out.push_str("=\"");
            escape(out, value, true);
            out.push('"');
        }
        if self.text.is_empty() && self.children.is_empty() {
            out.push_str(" />");
        } else {
            out.push('>');
            escape(out, &self.text, false);
            for child in &self.children {
                match child {
                    Node::Element(element) => element.write(out),
                    Node::Comment { text, tail } => {
                        out.push_str("<!--");
                        out.push_str(text);
                        out.push_str("-->");
                        escape(out, tail, false);
                    }
                    Node::Instruction { text, tail } => {
                        out.push_str("<?");
                        out.push_str(text);
                        out.push_str("?>");
                        escape(out, tail, false);
                    }
                }
            }
            out.push_str("</");
            out.push_str(&self.tag);
            out.push('>');
        }
        escape(out, &self.tail, false);
    }
}

fn escape(out: &mut String, text: &str, attribute: bool) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\r' if attribute => out.push_str("&#13;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\t' if attribute => out.push_str("&#09;"),
            c => out.push(c),
        }
    }
}

/// `b"<!DOCTYPE" in raw.upper() or b"<!ENTITY" in raw.upper()`.
pub fn has_declarations(raw: &[u8]) -> bool {
    let upper = raw.to_ascii_uppercase();
    [&b"<!DOCTYPE"[..], &b"<!ENTITY"[..]]
        .iter()
        .any(|needle| upper.windows(needle.len()).any(|window| window == *needle))
}

fn invalid(detail: impl std::fmt::Display) -> Error {
    Error::other(format!("not well-formed XML ({detail})"))
}

fn utf16(bytes: &[u8], little: bool) -> Result<String> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(invalid("truncated UTF-16"));
    }
    let units = pairs.iter().map(|pair| {
        if little {
            u16::from_le_bytes(*pair)
        } else {
            u16::from_be_bytes(*pair)
        }
    });
    char::decode_utf16(units)
        .collect::<std::result::Result<String, _>>()
        .map_err(|_| invalid("invalid UTF-16"))
}

fn declared_encoding(raw: &[u8]) -> Option<String> {
    if !raw.starts_with(b"<?xml") {
        return None;
    }
    let end = raw.windows(2).position(|window| window == b"?>")?;
    let declaration = String::from_utf8_lossy(&raw[..end]).into_owned();
    let rest = &declaration[declaration.find("encoding")? + "encoding".len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let quote = rest
        .chars()
        .next()
        .filter(|quote| matches!(quote, '"' | '\''))?;
    let value = &rest[1..];
    Some(value[..value.find(quote)?].to_ascii_lowercase())
}

const WINDOWS_1252: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
];

/// The document as text, by byte order mark or declared encoding like expat.
fn decode(raw: &[u8]) -> Result<String> {
    if let Some(rest) = raw.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(rest.to_vec()).map_err(|_| invalid("invalid UTF-8"));
    }
    if let Some(rest) = raw.strip_prefix(&[0xff, 0xfe]) {
        return utf16(rest, true);
    }
    if let Some(rest) = raw.strip_prefix(&[0xfe, 0xff]) {
        return utf16(rest, false);
    }
    if raw.starts_with(&[b'<', 0]) {
        return utf16(raw, true);
    }
    if raw.starts_with(&[0, b'<']) {
        return utf16(raw, false);
    }
    match declared_encoding(raw).as_deref() {
        None | Some("utf-8" | "utf8") => {
            String::from_utf8(raw.to_vec()).map_err(|_| invalid("invalid UTF-8"))
        }
        Some("us-ascii" | "ascii") if raw.is_ascii() => {
            Ok(String::from_utf8_lossy(raw).into_owned())
        }
        Some("iso-8859-1" | "iso8859-1" | "latin-1" | "latin1") => {
            Ok(raw.iter().map(|byte| char::from(*byte)).collect())
        }
        Some("windows-1252" | "cp1252") => Ok(raw
            .iter()
            .map(|byte| match byte {
                0x80..=0x9f => WINDOWS_1252[usize::from(byte - 0x80)],
                other => char::from(*other),
            })
            .collect()),
        Some(other) => Err(invalid(format!("unsupported encoding {other}"))),
    }
}

fn valid_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | ' '..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..)
}

fn valid_name(name: &str) -> bool {
    let start = |c: char| c.is_alphabetic() || matches!(c, '_' | ':') || !c.is_ascii();
    let mut chars = name.chars();
    chars.next().is_some_and(start)
        && chars.all(|c| start(c) || c.is_ascii_digit() || matches!(c, '-' | '.'))
}

/// `ET.fromstring(raw)`.
pub fn parse(raw: &[u8]) -> Result<Element> {
    // XML defines line ends before anything else is read: CR LF and CR become LF.
    let text = decode(raw)?.replace("\r\n", "\n").replace('\r', "\n");
    if let Some(bad) = text.chars().find(|c| !valid_char(*c)) {
        return Err(invalid(format!(
            "invalid character U+{:04X}",
            u32::from(bad)
        )));
    }
    let mut reader = Reader::from_str(&text);
    reader.config_mut().check_comments = true;
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut first = true;
    let data = |stack: &mut Vec<Element>, root: &Option<Element>, text: &str| -> Result<()> {
        match stack.last_mut() {
            Some(element) => match element.children.last_mut() {
                Some(child) => child.tail_mut().push_str(text),
                None => element.text.push_str(text),
            },
            None if text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')) => {}
            None if root.is_some() => return Err(invalid("junk after document element")),
            None => return Err(invalid("syntax error")),
        }
        Ok(())
    };
    let open = |start: &quick_xml::events::BytesStart<'_>| -> Result<Element> {
        let tag = start.name().as_ref().to_string();
        if !valid_name(&tag) {
            return Err(invalid("invalid element name"));
        }
        let mut element = Element::new(&tag);
        for attribute in start.attributes() {
            let attribute = attribute.map_err(invalid)?;
            let key = attribute.key.as_ref().to_string();
            let value = attribute
                .normalized_value(XmlVersion::Explicit1_0)
                .map_err(invalid)?
                .into_owned();
            if !valid_name(&key) || !value.chars().all(valid_char) {
                return Err(invalid("invalid attribute"));
            }
            element.attributes.push((key, value));
        }
        Ok(element)
    };
    let close = |stack: &mut Vec<Element>, root: &mut Option<Element>, element: Element| match stack
        .last_mut()
    {
        Some(parent) => parent.children.push(Node::Element(element)),
        None => *root = Some(element),
    };
    loop {
        let event = reader.read_event().map_err(invalid)?;
        let was_first = std::mem::replace(&mut first, false);
        match event {
            Event::Start(start) => {
                if stack.is_empty() && root.is_some() {
                    return Err(invalid("junk after document element"));
                }
                if stack.len() >= MAX_DEPTH {
                    return Err(invalid("elements are nested too deeply"));
                }
                stack.push(open(&start)?);
            }
            Event::Empty(start) => {
                if stack.is_empty() && root.is_some() {
                    return Err(invalid("junk after document element"));
                }
                let element = open(&start)?;
                close(&mut stack, &mut root, element);
            }
            Event::End(_) => {
                let element = stack.pop().ok_or_else(|| invalid("mismatched tag"))?;
                close(&mut stack, &mut root, element);
            }
            Event::Text(content) => data(&mut stack, &root, &content.into_inner())?,
            Event::CData(content) => {
                if stack.is_empty() {
                    return Err(invalid("syntax error"));
                }
                data(&mut stack, &root, &content.into_inner())?;
            }
            Event::GeneralRef(reference) => {
                let resolved = match reference.resolve_char_ref().map_err(invalid)? {
                    Some(c) if valid_char(c) => c.to_string(),
                    Some(_) => return Err(invalid("reference to invalid character number")),
                    None => resolve_predefined_entity(&reference)
                        .ok_or_else(|| invalid("undefined entity"))?
                        .to_string(),
                };
                if stack.is_empty() {
                    return Err(invalid("syntax error"));
                }
                data(&mut stack, &root, &resolved)?;
            }
            Event::Comment(content) => {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Comment {
                        text: content.into_inner().into_owned(),
                        tail: String::new(),
                    });
                }
            }
            Event::PI(content) => {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Instruction {
                        text: content.into_inner().into_owned(),
                        tail: String::new(),
                    });
                }
            }
            Event::Decl(_) if was_first => {}
            Event::Decl(_) => {
                return Err(invalid("XML or text declaration not at start of entity"));
            }
            Event::DocType(_) => {
                return Err(invalid("document type declarations are not supported"));
            }
            Event::Eof => break,
        }
    }
    if !stack.is_empty() {
        return Err(invalid("no element found"));
    }
    root.ok_or_else(|| invalid("no element found"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn round(input: &str) -> String {
        String::from_utf8(parse(input.as_bytes()).unwrap().to_bytes()).unwrap()
    }

    const DECLARATION: &str = "<?xml version='1.0' encoding='utf-8'?>\n";

    #[test]
    fn output_matches_elementtree() {
        // Each expectation is what Python 3 prints for the same input.
        for (input, expected) in [
            ("<a/>", "<a />"),
            ("<a></a>", "<a />"),
            (
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<a>\r\n  <b x='1' y=\"&quot;&lt;&#10;\tq\">t &amp; &#x41;&gt;]]&gt;</b>\r\n</a>\r\n",
                "<a>\n  <b x=\"1\" y=\"&quot;&lt;&#10; q\">t &amp; A&gt;]]&gt;</b>\n</a>",
            ),
            ("<a><![CDATA[<x> & y]]>z</a>", "<a>&lt;x&gt; &amp; yz</a>"),
            (
                "<a> <b/> tail <c>&apos;\"</c></a>\n\n",
                "<a> <b /> tail <c>'\"</c></a>",
            ),
            ("\u{feff}<a>\u{e4}&#228;</a>", "<a>\u{e4}\u{e4}</a>"),
        ] {
            assert_eq!(round(input), format!("{DECLARATION}{expected}"), "{input}");
        }
    }

    #[test]
    fn comments_and_instructions_inside_the_root_are_kept() {
        assert_eq!(
            round("<!-- top --><a><!-- c -->x<?pi data?>y<b/></a><!-- end -->"),
            format!("{DECLARATION}<a><!-- c -->x<?pi data?>y<b /></a>")
        );
    }

    #[test]
    fn other_encodings_are_read() {
        let mut utf16 = vec![0xff, 0xfe];
        utf16.extend("<a>\u{e4}</a>".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(parse(&utf16).unwrap().text, "\u{e4}");
        let latin = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>\xe4</a>";
        assert_eq!(parse(latin).unwrap().text, "\u{e4}");
        let windows = b"<?xml version='1.0' encoding='windows-1252'?><a>\x80</a>";
        assert_eq!(parse(windows).unwrap().text, "\u{20ac}");
    }

    #[test]
    fn editing_follows_elementtree() {
        let mut root = parse(
            b"<Config>\n  <displayMode>GPU</displayMode>\n  <keep a=\"1\">x</keep>\n</Config>",
        )
        .unwrap();
        root.set("displayMode", "CPU");
        root.set("newRender", "false");
        assert_eq!(root.findtext("displayMode"), "CPU");
        assert_eq!(root.findtext("missing"), "");
        assert_eq!(
            String::from_utf8(root.to_bytes()).unwrap(),
            format!(
                "{DECLARATION}<Config>\n  <displayMode>CPU</displayMode>\n  <keep a=\"1\">x</keep>\n<newRender>false</newRender></Config>"
            )
        );
    }

    #[test]
    fn malformed_documents_are_rejected() {
        for input in [
            "",
            "<a>",
            "<a></b>",
            "<a/><b/>",
            "text<a/>",
            "<a/>text",
            "<a x=1/>",
            "<a x='1' x='2'/>",
            "<a>&unknown;</a>",
            "<a>&</a>",
            "<!DOCTYPE a><a/>",
            "<a>\u{1}</a>",
            "<a>&#0;</a>",
            "<a><?xml version='1.0'?></a>",
            "<1a/>",
            "<a><!-- -- --></a>",
        ] {
            assert!(parse(input.as_bytes()).is_err(), "{input}");
        }
        assert!(has_declarations(b"<!doctype x>"));
        assert!(has_declarations(b"..<!Entity"));
        assert!(!has_declarations(b"<a>DOCTYPE</a>"));
    }
}
