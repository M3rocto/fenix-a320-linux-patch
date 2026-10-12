// SPDX-License-Identifier: MIT
//! Minimal, position-preserving edits of Steam's config.vdf.
//!
//! Only the simulator's compatibility-tool entry is touched; every other byte is
//! kept. Anything this small parser does not understand is rejected, so the file
//! is then left alone.
use crate::{
    error::{Error, Result},
    py,
};
use regex::Regex;
use std::sync::LazyLock;

// Python's `\s` additionally covers the four separator control characters.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| py::pattern(r#"\A(?:[\s\x1c-\x1f]|//[^\n]*)*(?:"((?:[^"\\]|\\.)*)"|([{}]))"#));
const STEAM: [&str; 4] = ["InstallConfigStore", "Software", "Valve", "Steam"];
const MAPPING: &str = "CompatToolMapping";
const UNREADABLE: &str = "Steam's configuration could not be read.";
const LAYOUT: &str = "Steam's configuration has an unexpected layout.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Text(String),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: Item,
    /// Offset of the key's opening quote.
    pub start: usize,
    /// Offset just behind the value.
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub entries: Vec<Entry>,
    pub start: usize,
    /// Offset of the closing brace.
    pub close: usize,
    pub end: usize,
}

/// The structure without positions, for comparing two documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plain {
    Text(String),
    Block(Vec<(String, Plain)>),
}

impl Block {
    fn find(&self, key: &str) -> Result<Option<&Entry>> {
        let wanted = key.to_lowercase();
        let mut found = self
            .entries
            .iter()
            .filter(|entry| entry.key.to_lowercase() == wanted);
        let first = found.next();
        if found.next().is_some() {
            return Err(Error::patch(format!(
                "Steam's configuration has duplicate entries: {key}"
            )));
        }
        Ok(first)
    }
}

struct Token<'a> {
    text: Option<&'a str>,
    brace: Option<u8>,
    /// Start of the quoted text or of the brace.
    start: usize,
    end: usize,
}

fn token(text: &str, position: usize) -> Option<Token<'_>> {
    let found = TOKEN.captures(text.get(position..)?)?;
    let end = position + found.get(0)?.end();
    match (found.get(1), found.get(2)) {
        (Some(quoted), _) => Some(Token {
            text: Some(quoted.as_str()),
            brace: None,
            start: position + quoted.start(),
            end,
        }),
        (None, Some(brace)) => Some(Token {
            text: None,
            brace: brace.as_str().bytes().next(),
            start: position + brace.start(),
            end,
        }),
        (None, None) => None,
    }
}

fn entries(
    text: &str,
    mut position: usize,
    top: bool,
    depth: usize,
) -> Result<(Vec<Entry>, usize, usize)> {
    let unreadable = || Error::patch(UNREADABLE);
    // Python gave up with a RecursionError on absurd nesting.
    if depth > 200 {
        return Err(unreadable());
    }
    let mut found = Vec::new();
    loop {
        let Some(first) = token(text, position) else {
            if top && py::strip(&text[position..]).is_empty() {
                return Ok((found, text.len(), text.len()));
            }
            return Err(unreadable());
        };
        if first.brace == Some(b'}') && !top {
            return Ok((found, first.start, first.end));
        }
        let Some(key) = first.text else {
            return Err(unreadable());
        };
        let start = first.start - 1;
        let second = token(text, first.end)
            .filter(|second| second.brace != Some(b'}'))
            .ok_or_else(unreadable)?;
        let value = match second.text {
            None => {
                let (inner, close, end) = entries(text, second.end, false, depth + 1)?;
                position = end;
                Item::Block(Block {
                    entries: inner,
                    start: second.end,
                    close,
                    end,
                })
            }
            Some(value) => {
                position = second.end;
                Item::Text(value.to_string())
            }
        };
        found.push(Entry {
            key: key.to_string(),
            value,
            start,
            end: position,
        });
    }
}

pub fn parse(text: &str) -> Result<Block> {
    let (found, _, _) = entries(text, 0, true, 0)?;
    Ok(Block {
        entries: found,
        start: 0,
        close: text.len(),
        end: text.len(),
    })
}

pub fn plain(block: &Block) -> Vec<(String, Plain)> {
    block
        .entries
        .iter()
        .map(|entry| {
            let value = match &entry.value {
                Item::Block(inner) => Plain::Block(plain(inner)),
                Item::Text(text) => Plain::Text(text.clone()),
            };
            (entry.key.clone(), value)
        })
        .collect()
}

fn steam_block(root: &Block) -> Result<&Block> {
    let mut block = root;
    for key in STEAM {
        match block.find(key)? {
            Some(Entry {
                value: Item::Block(inner),
                ..
            }) => block = inner,
            _ => return Err(Error::patch(LAYOUT)),
        }
    }
    Ok(block)
}

fn mapping_block(root: &Block) -> Result<Option<&Block>> {
    match steam_block(root)?.find(MAPPING)? {
        None => Ok(None),
        Some(Entry {
            value: Item::Block(inner),
            ..
        }) => Ok(Some(inner)),
        Some(_) => Err(Error::patch(LAYOUT)),
    }
}

/// The forced compatibility tool for one application, or None.
pub fn compat_tool(text: &str, appid: &str) -> Result<Option<String>> {
    let root = parse(text)?;
    let Some(mapping) = mapping_block(&root)? else {
        return Ok(None);
    };
    let Some(Entry {
        value: Item::Block(entry),
        ..
    }) = mapping.find(appid)?
    else {
        return Ok(None);
    };
    Ok(match entry.find("name")? {
        Some(Entry {
            value: Item::Text(name),
            ..
        }) if !name.is_empty() => Some(name.clone()),
        _ => None,
    })
}

fn line_start(text: &str, position: usize) -> Result<usize> {
    let start = text[..position].rfind('\n').map_or(0, |index| index + 1);
    if !py::strip(&text[start..position]).is_empty() {
        return Err(Error::patch(LAYOUT));
    }
    Ok(start)
}

/// Structure with this application's entry (and an emptied mapping) left out.
fn without(text: &str, appid: &str) -> Result<Vec<(String, Plain)>> {
    fn strip(items: Vec<(String, Plain)>, path: &[String], appid: &str) -> Vec<(String, Plain)> {
        let steam: Vec<String> = STEAM.iter().map(|part| part.to_lowercase()).collect();
        let mut result = Vec::new();
        for (key, value) in items {
            let value = match value {
                Plain::Block(inner) => {
                    let mut here = path.to_vec();
                    here.push(key.to_lowercase());
                    let below_steam = here.len() > steam.len()
                        && here[..steam.len()] == steam[..]
                        && here[steam.len()] == MAPPING.to_lowercase();
                    if below_steam
                        && here.len() == steam.len() + 2
                        && here[steam.len() + 1] == appid
                    {
                        continue;
                    }
                    let inner = strip(inner, &here, appid);
                    if below_steam && here.len() == steam.len() + 1 && inner.is_empty() {
                        continue;
                    }
                    Plain::Block(inner)
                }
                text => text,
            };
            result.push((key, value));
        }
        result
    }
    Ok(strip(plain(&parse(text)?), &[], appid))
}

static APPLICATION: LazyLock<Regex> = LazyLock::new(|| py::pattern(r"\A\d+\z"));
static TOOL_NAME: LazyLock<Regex> = LazyLock::new(|| py::pattern(r"\A[A-Za-z0-9._ -]+\z"));

/// Force `tool` for `appid`; with tool None, remove the application's entry.
pub fn set_compat_tool(text: &str, appid: &str, tool: Option<&str>) -> Result<String> {
    if !APPLICATION.is_match(appid) || tool.is_some_and(|tool| !TOOL_NAME.is_match(tool)) {
        return Err(Error::patch("Invalid compatibility tool selection"));
    }
    let root = parse(text)?;
    let steam = steam_block(&root)?;
    let mapping = mapping_block(&root)?;
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let entry = |depth: usize, tool: &str| {
        let tabs = "\t".repeat(depth);
        [
            format!("{tabs}\"{appid}\""),
            format!("{tabs}{{"),
            format!("{tabs}\t\"name\"\t\t\"{tool}\""),
            format!("{tabs}\t\"config\"\t\t\"\""),
            format!("{tabs}\t\"priority\"\t\t\"250\""),
            format!("{tabs}}}"),
        ]
        .join(newline)
            + newline
    };
    let depth = STEAM.len() + 1;
    let mut existing = match mapping {
        Some(mapping) => mapping.find(appid)?,
        None => None,
    };
    if existing.is_some()
        && tool.is_none()
        && mapping.is_some_and(|mapping| mapping.entries.len() == 1)
    {
        // Leave no empty mapping behind when the only entry goes.
        existing = steam.find(MAPPING)?;
    }
    let result = if let Some(existing) = existing {
        let start = line_start(text, existing.start)?;
        let end = text[existing.end - 1..]
            .find('\n')
            .map_or(text.len(), |index| existing.end - 1 + index + 1);
        format!(
            "{}{}{}",
            &text[..start],
            tool.map(|tool| entry(depth, tool)).unwrap_or_default(),
            &text[end..]
        )
    } else if let Some(tool) = tool {
        if let Some(mapping) = mapping {
            let start = line_start(text, mapping.close)?;
            format!("{}{}{}", &text[..start], entry(depth, tool), &text[start..])
        } else {
            let start = line_start(text, steam.close)?;
            let tabs = "\t".repeat(depth - 1);
            format!(
                "{}{tabs}\"{MAPPING}\"{newline}{tabs}{{{newline}{}{tabs}}}{newline}{}",
                &text[..start],
                entry(depth, tool),
                &text[start..]
            )
        }
    } else {
        return Ok(text.to_string());
    };
    // Everything else must be exactly what Steam wrote.
    if without(&result, appid)? != without(text, appid)?
        || compat_tool(&result, appid)?.as_deref() != tool
    {
        return Err(Error::patch(
            "Steam's configuration could not be changed safely; select the tool in Steam instead.",
        ));
    }
    Ok(result)
}
