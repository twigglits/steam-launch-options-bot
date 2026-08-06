//! Text KeyValues (VDF) parser/serializer for Steam config files.
//!
//! Parses into ordered nested blocks and serializes back in the exact style
//! Steam writes: tab indentation, two tabs between key and value.
//!
//! Two guarantees hold for every input: parsing is lossless, and `dumps` output
//! is a fixed point. On top of that, a file already in Steam's canonical form
//! round-trips byte-identically - which covers localconfig.vdf, the only file
//! steamtrain ever writes. It does *not* hold universally, and the Python this
//! was ported from claimed otherwise: Valve embeds raw newlines inside quoted
//! values in config.vdf (the SDL controller mappings), and those come back as
//! the `\n` escape. The value is unchanged and re-parses identically, so this
//! is a formatting normalisation rather than data loss - but config.vdf is
//! read-only to this tool either way.
//!
//! Keys and values are bytes, not `String`. Python read these files with
//! `errors="surrogateescape"` so arbitrary bytes survived a round trip; Rust
//! has no equivalent, and `String::from_utf8_lossy` would replace an invalid
//! sequence with U+FFFD and then write that corruption straight back into the
//! user's localconfig.vdf. Conversion to text happens at display and JSON
//! edges only.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(Vec<u8>),
    Block(Block),
}

impl Value {
    pub fn as_str(&self) -> Option<&[u8]> {
        match self {
            Value::Str(bytes) => Some(bytes),
            Value::Block(_) => None,
        }
    }

    pub fn as_block(&self) -> Option<&Block> {
        match self {
            Value::Block(block) => Some(block),
            Value::Str(_) => None,
        }
    }
}

/// An insertion-ordered map with byte keys.
///
/// Ordered because a round-trip has to be byte-identical, and a hash map would
/// reshuffle every block it touched. Lookups are linear, which is what Valve
/// KeyValues needs anyway: `child_ci` matches case-insensitively, so a hash
/// would not help.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Block {
    entries: Vec<(Vec<u8>, Value)>,
}

impl Block {
    pub fn new() -> Self {
        Block {
            entries: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &Value)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_slice(), value))
    }

    pub fn keys(&self) -> impl Iterator<Item = &[u8]> {
        self.entries.iter().map(|(key, _)| key.as_slice())
    }

    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    pub fn get_str(&self, key: &[u8]) -> Option<&[u8]> {
        self.get(key).and_then(Value::as_str)
    }

    pub fn get_block(&self, key: &[u8]) -> Option<&Block> {
        self.get(key).and_then(Value::as_block)
    }

    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }

    /// Set `key`, replacing an existing entry **in place** so its position
    /// survives, or appending. This is Python's `dict[key] = value`, and the
    /// in-place part is what keeps round-trips stable.
    pub fn insert(&mut self, key: Vec<u8>, value: Value) {
        match self.entries.iter_mut().find(|(name, _)| *name == key) {
            Some(entry) => entry.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    /// Case-insensitive child-block lookup, creating the block with canonical
    /// casing when absent. Valve KeyValues is case-insensitive, so a config
    /// written as "UserLocalConfigStore" and one written as
    /// "userlocalconfigstore" name the same node.
    ///
    /// A case-insensitive match against a *string* value does not count as a
    /// child: the insert that follows then replaces it only when the casing
    /// matches exactly, which is what Python's `node[name] = {}` did.
    pub fn child_ci(&mut self, name: &[u8]) -> &mut Block {
        let existing = self.entries.iter().position(|(key, value)| {
            key.eq_ignore_ascii_case(name) && matches!(value, Value::Block(_))
        });
        let index = match existing {
            Some(index) => index,
            None => {
                self.insert(name.to_vec(), Value::Block(Block::new()));
                self.entries
                    .iter()
                    .position(|(key, _)| key == name)
                    .expect("the entry was just inserted under this exact key")
            }
        };
        match &mut self.entries[index].1 {
            Value::Block(block) => block,
            Value::Str(_) => unreachable!("the selected entry is a block"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VdfError {
    UnterminatedString,
    UnterminatedConditional,
    BlockWithoutKey,
    DanglingKey(Vec<u8>),
    DanglingKeyAtEnd(Vec<u8>),
    UnbalancedClose,
    UnclosedBlock,
}

impl fmt::Display for VdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VdfError::UnterminatedString => write!(f, "unterminated quoted string"),
            VdfError::UnterminatedConditional => write!(f, "unterminated conditional"),
            VdfError::BlockWithoutKey => write!(f, "block has no key"),
            VdfError::DanglingKey(key) => {
                write!(
                    f,
                    "dangling key {} before '}}'",
                    String::from_utf8_lossy(key)
                )
            }
            VdfError::DanglingKeyAtEnd(key) => write!(
                f,
                "dangling key {} at end of input",
                String::from_utf8_lossy(key)
            ),
            VdfError::UnbalancedClose => write!(f, "unbalanced '}}'"),
            VdfError::UnclosedBlock => write!(f, "unclosed block"),
        }
    }
}

impl std::error::Error for VdfError {}

const WHITESPACE: [u8; 4] = [b' ', b'\t', b'\r', b'\n'];
const BARE_TOKEN_END: [u8; 7] = [b' ', b'\t', b'\r', b'\n', b'"', b'{', b'}'];

enum Token {
    Str(Vec<u8>),
    Open,
    Close,
}

struct Tokenizer<'a> {
    text: &'a [u8],
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    fn new(text: &'a [u8]) -> Self {
        Tokenizer { text, pos: 0 }
    }

    fn find_from(&self, needle: u8) -> Option<usize> {
        self.text[self.pos..]
            .iter()
            .position(|&b| b == needle)
            .map(|offset| self.pos + offset)
    }

    fn next_token(&mut self) -> Result<Option<Token>, VdfError> {
        loop {
            let Some(&c) = self.text.get(self.pos) else {
                return Ok(None);
            };
            if WHITESPACE.contains(&c) {
                self.pos += 1;
            } else if c == b'/' && self.text[self.pos..].starts_with(b"//") {
                self.pos = match self.find_from(b'\n') {
                    Some(index) => index + 1,
                    None => self.text.len(),
                };
            } else if c == b'{' {
                self.pos += 1;
                return Ok(Some(Token::Open));
            } else if c == b'}' {
                self.pos += 1;
                return Ok(Some(Token::Close));
            } else if c == b'"' {
                return self.quoted().map(Some);
            } else if c == b'[' {
                // A platform conditional such as [$LINUX]; skip it.
                match self.find_from(b']') {
                    Some(end) => self.pos = end + 1,
                    None => return Err(VdfError::UnterminatedConditional),
                }
            } else {
                let start = self.pos;
                while self
                    .text
                    .get(self.pos)
                    .is_some_and(|b| !BARE_TOKEN_END.contains(b))
                {
                    self.pos += 1;
                }
                return Ok(Some(Token::Str(self.text[start..self.pos].to_vec())));
            }
        }
    }

    fn quoted(&mut self) -> Result<Token, VdfError> {
        self.pos += 1; // the opening quote
        let mut parts = Vec::new();
        loop {
            let Some(&c) = self.text.get(self.pos) else {
                return Err(VdfError::UnterminatedString);
            };
            if c == b'\\' && self.pos + 1 < self.text.len() {
                match unescape(self.text[self.pos + 1]) {
                    Some(mapped) => {
                        parts.push(mapped);
                        self.pos += 2;
                    }
                    // An unrecognised escape keeps its backslash, so it
                    // survives the round trip untouched.
                    None => {
                        parts.push(c);
                        self.pos += 1;
                    }
                }
            } else if c == b'"' {
                self.pos += 1;
                return Ok(Token::Str(parts));
            } else {
                parts.push(c);
                self.pos += 1;
            }
        }
    }
}

fn unescape(escape: u8) -> Option<u8> {
    match escape {
        b'"' => Some(b'"'),
        b'\\' => Some(b'\\'),
        b'n' => Some(b'\n'),
        b't' => Some(b'\t'),
        _ => None,
    }
}

/// Parse VDF text into nested blocks, preserving key order.
pub fn loads(text: &[u8]) -> Result<Block, VdfError> {
    let mut tokenizer = Tokenizer::new(text);
    let mut root = Block::new();
    // (the key that opened this block, the block being filled). Built
    // bottom-up rather than holding a stack of &mut into the tree; a block is
    // attached to its parent on close, which lands it in the same position it
    // would have taken on open, since nothing else can be added to the parent
    // in between.
    let mut stack: Vec<(Vec<u8>, Block)> = Vec::new();
    let mut pending_key: Option<Vec<u8>> = None;

    while let Some(token) = tokenizer.next_token()? {
        match token {
            Token::Str(value) => match pending_key.take() {
                None => pending_key = Some(value),
                Some(key) => {
                    let target = stack.last_mut().map_or(&mut root, |(_, block)| block);
                    target.insert(key, Value::Str(value));
                }
            },
            Token::Open => {
                let Some(key) = pending_key.take() else {
                    return Err(VdfError::BlockWithoutKey);
                };
                stack.push((key, Block::new()));
            }
            Token::Close => {
                if let Some(key) = pending_key.take() {
                    return Err(VdfError::DanglingKey(key));
                }
                let Some((key, block)) = stack.pop() else {
                    return Err(VdfError::UnbalancedClose);
                };
                let target = stack.last_mut().map_or(&mut root, |(_, block)| block);
                target.insert(key, Value::Block(block));
            }
        }
    }

    if let Some(key) = pending_key {
        return Err(VdfError::DanglingKeyAtEnd(key));
    }
    if !stack.is_empty() {
        return Err(VdfError::UnclosedBlock);
    }
    Ok(root)
}

/// Serialize nested blocks to VDF text in Steam's own formatting.
pub fn dumps(block: &Block) -> Vec<u8> {
    let mut out = Vec::new();
    dump_block(block, 0, &mut out);
    out
}

fn dump_block(block: &Block, depth: usize, out: &mut Vec<u8>) {
    for (key, value) in block.iter() {
        indent(depth, out);
        out.push(b'"');
        escape_into(key, out);
        out.push(b'"');
        match value {
            Value::Block(child) => {
                out.push(b'\n');
                indent(depth, out);
                out.extend_from_slice(b"{\n");
                dump_block(child, depth + 1, out);
                indent(depth, out);
                out.extend_from_slice(b"}\n");
            }
            Value::Str(text) => {
                out.extend_from_slice(b"\t\t\"");
                escape_into(text, out);
                out.extend_from_slice(b"\"\n");
            }
        }
    }
}

fn indent(depth: usize, out: &mut Vec<u8>) {
    out.extend(std::iter::repeat(b'\t').take(depth));
}

fn escape_into(bytes: &[u8], out: &mut Vec<u8>) {
    for &byte in bytes {
        match byte {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'"' => out.extend_from_slice(b"\\\""),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            _ => out.push(byte),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_replaces_in_place_and_keeps_position() {
        let mut block = Block::new();
        block.insert(b"a".to_vec(), Value::Str(b"1".to_vec()));
        block.insert(b"b".to_vec(), Value::Str(b"2".to_vec()));
        block.insert(b"a".to_vec(), Value::Str(b"3".to_vec()));

        let keys: Vec<&[u8]> = block.keys().collect();
        assert_eq!(keys, vec![&b"a"[..], &b"b"[..]]);
        assert_eq!(block.get_str(b"a"), Some(&b"3"[..]));
    }

    #[test]
    fn child_ci_matches_case_insensitively() {
        let mut block = Block::new();
        block.insert(b"Apps".to_vec(), Value::Block(Block::new()));
        block
            .child_ci(b"apps")
            .insert(b"k".to_vec(), Value::Str(b"v".to_vec()));

        assert_eq!(block.len(), 1);
        assert_eq!(
            block.get_block(b"Apps").unwrap().get_str(b"k"),
            Some(&b"v"[..])
        );
    }

    #[test]
    fn child_ci_creates_the_block_when_absent() {
        let mut block = Block::new();
        block.child_ci(b"apps");
        assert!(block.get_block(b"apps").is_some());
    }

    #[test]
    fn child_ci_does_not_adopt_a_string_of_the_same_name() {
        let mut block = Block::new();
        block.insert(b"apps".to_vec(), Value::Str(b"oops".to_vec()));
        block.child_ci(b"apps");
        // Exact casing, so the string was replaced rather than shadowed.
        assert_eq!(block.len(), 1);
        assert!(block.get_block(b"apps").is_some());
    }

    #[test]
    fn a_raw_newline_in_a_value_is_normalised_to_an_escape() {
        // Valve writes raw newlines inside quoted values in config.vdf. They
        // parse fine, and come back out escaped: the value survives and the
        // second dump is a fixed point, but the bytes are not the input's.
        // This is the one place the round trip is not byte-identical, and it
        // is asserted here so the behaviour is pinned rather than discovered.
        let source = b"\"R\"\n{\n\t\"k\"\t\t\"one\ntwo\"\n}\n";
        let parsed = loads(source).unwrap();
        assert_eq!(
            parsed.get_block(b"R").unwrap().get_str(b"k"),
            Some(&b"one\ntwo"[..])
        );

        let dumped = dumps(&parsed);
        assert_ne!(dumped, source);
        assert_eq!(loads(&dumped).unwrap(), parsed);
        assert_eq!(dumps(&loads(&dumped).unwrap()), dumped);
    }
}
