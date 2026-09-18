//! The System V / GNU `ar` archive writer (and just enough of a reader to check one).
//!
//! An archive is the plainest container in the toolchain: the eight bytes `!<arch>\n`, then
//! members, each with a 60-byte header of space-padded decimal ASCII, each padded to an even
//! offset. Two members are special and come first:
//!
//! - `/` — the **symbol index**: a big-endian `u32` count, that many big-endian `u32` file
//!   offsets of member *headers*, then that many NUL-terminated symbol names. This is what
//!   lets a linker answer "which member defines `foo`?" without reading every member.
//! - `//` — the **long name table**: names longer than 15 characters, `/`-terminated and
//!   newline-separated; a member header then carries `/<decimal offset>` instead of a name.
//!
//! Short member names are written with a trailing `/` (`foo.o/`), which is how GNU `ar`
//! allows a name to end in a space.

use std::fmt::Write as _;

/// The magic every archive starts with.
pub const ARCHIVE_MAGIC: &[u8; 8] = b"!<arch>\n";
/// The two bytes that end every member header.
pub const HEADER_TERMINATOR: &[u8; 2] = b"`\n";
/// Size of a member header.
pub const MEMBER_HEADER_SIZE: usize = 60;

/// One member waiting to be written.
#[derive(Debug, Clone)]
pub struct Member {
    /// The file name recorded in the archive (`foo.o`).
    pub name: String,
    /// The member's bytes — a relocatable object, for this suite.
    pub data: Vec<u8>,
    /// The global symbols the index should advertise for this member.
    pub symbols: Vec<String>,
}

impl Member {
    /// A member with the symbols its index entry should carry.
    pub fn new(name: &str, data: Vec<u8>, symbols: &[&str]) -> Member {
        Member {
            name: name.to_string(),
            data,
            symbols: symbols.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

/// How the archive's symbol index should be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndexMode {
    /// A correct `/` member listing every symbol at the right member offset.
    #[default]
    Correct,
    /// No `/` member at all — a linker has to scan the members' own symbol tables.
    Omitted,
    /// A `/` member whose offsets point at the wrong members (what `ar q` without `s`
    /// leaves behind once a member has moved).
    Stale,
    /// A `/` member that mentions symbols no member defines.
    Phantom,
}

/// Builds one archive.
#[derive(Debug, Clone, Default)]
pub struct ArchiveBuilder {
    members: Vec<Member>,
    index: IndexMode,
    magic_override: Option<[u8; 8]>,
    truncate_to: Option<usize>,
}

impl ArchiveBuilder {
    /// An empty archive.
    pub fn new() -> ArchiveBuilder {
        ArchiveBuilder::default()
    }

    /// Append a member. Order is the order a linker scans in.
    pub fn member(mut self, m: Member) -> ArchiveBuilder {
        self.members.push(m);
        self
    }

    /// How to write the symbol index.
    pub fn index(mut self, mode: IndexMode) -> ArchiveBuilder {
        self.index = mode;
        self
    }

    /// Corrupt the `!<arch>\n` magic.
    pub fn magic_override(mut self, magic: [u8; 8]) -> ArchiveBuilder {
        self.magic_override = Some(magic);
        self
    }

    /// Cut the finished archive short.
    pub fn truncate_to(mut self, n: usize) -> ArchiveBuilder {
        self.truncate_to = Some(n);
        self
    }

    /// The finished archive.
    pub fn build(&self) -> Result<Vec<u8>, String> {
        // Long names go into the `//` member; short ones are inline with a trailing `/`.
        let mut long_names = Vec::<u8>::new();
        let mut stored_names = Vec::<String>::new();
        for m in &self.members {
            if m.name.len() <= 15 {
                stored_names.push(format!("{}/", m.name));
            } else {
                let at = long_names.len();
                long_names.extend_from_slice(m.name.as_bytes());
                long_names.extend_from_slice(b"/\n");
                stored_names.push(format!("/{at}"));
            }
        }

        // Two passes: the index has to know where every member header lands, and the index
        // itself moves them, so lay out with a placeholder index of the right size first.
        let symbol_count: usize = self.members.iter().map(|m| m.symbols.len()).sum();
        let index_payload_len = if self.index == IndexMode::Omitted {
            0
        } else {
            let names: usize = self
                .members
                .iter()
                .flat_map(|m| m.symbols.iter())
                .map(|s| s.len() + 1)
                .sum();
            let phantom = if self.index == IndexMode::Phantom {
                "linktest_phantom_symbol".len() + 1 + 4
            } else {
                0
            };
            4 + 4 * symbol_count + names + phantom
        };

        let mut cursor = ARCHIVE_MAGIC.len();
        if index_payload_len > 0 {
            cursor += MEMBER_HEADER_SIZE + pad_even(index_payload_len);
        }
        if !long_names.is_empty() {
            cursor += MEMBER_HEADER_SIZE + pad_even(long_names.len());
        }
        let mut member_offsets = Vec::new();
        for m in &self.members {
            member_offsets.push(cursor as u32);
            cursor += MEMBER_HEADER_SIZE + pad_even(m.data.len());
        }

        let mut out = Vec::new();
        out.extend_from_slice(&self.magic_override.unwrap_or(*ARCHIVE_MAGIC));

        if index_payload_len > 0 {
            let mut payload = Vec::<u8>::new();
            let mut entries: Vec<(u32, &str)> = Vec::new();
            for (i, m) in self.members.iter().enumerate() {
                let offset = match self.index {
                    // A stale index points every symbol at the *first* member, which is the
                    // shape `ar` leaves behind when members are replaced without `ar s`.
                    IndexMode::Stale => *member_offsets.first().unwrap_or(&0),
                    _ => member_offsets[i],
                };
                for s in &m.symbols {
                    entries.push((offset, s.as_str()));
                }
            }
            if self.index == IndexMode::Phantom {
                entries.push((
                    *member_offsets.first().unwrap_or(&0),
                    "linktest_phantom_symbol",
                ));
            }
            payload.extend_from_slice(&(entries.len() as u32).to_be_bytes());
            for (offset, _) in &entries {
                payload.extend_from_slice(&offset.to_be_bytes());
            }
            for (_, name) in &entries {
                payload.extend_from_slice(name.as_bytes());
                payload.push(0);
            }
            write_member(&mut out, "/", &payload)?;
        }

        if !long_names.is_empty() {
            write_member(&mut out, "//", &long_names)?;
        }

        for (m, name) in self.members.iter().zip(stored_names.iter()) {
            write_member(&mut out, name, &m.data)?;
        }

        if let Some(n) = self.truncate_to {
            out.truncate(n);
        }
        Ok(out)
    }
}

fn pad_even(n: usize) -> usize {
    n + (n % 2)
}

/// Write one 60-byte member header and the payload, padded to an even offset.
fn write_member(out: &mut Vec<u8>, name: &str, data: &[u8]) -> Result<(), String> {
    if name.len() > 16 {
        return Err(format!("member name '{name}' does not fit a 16-byte field"));
    }
    let mut header = String::new();
    let _ = write!(header, "{name:<16}");
    let _ = write!(header, "{:<12}", 0); // mtime
    let _ = write!(header, "{:<6}", 0); // uid
    let _ = write!(header, "{:<6}", 0); // gid
    let _ = write!(header, "{:<8}", "644"); // mode, octal
    let _ = write!(header, "{:<10}", data.len());
    if header.len() != MEMBER_HEADER_SIZE - 2 {
        return Err(format!(
            "internal: member header came out {} bytes, not {}",
            header.len() + 2,
            MEMBER_HEADER_SIZE
        ));
    }
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(HEADER_TERMINATOR);
    out.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        out.push(b'\n');
    }
    Ok(())
}

/// A member found while reading an archive back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMember {
    /// The member's name, `/`-suffix stripped and long names resolved.
    pub name: String,
    /// File offset of the member's header.
    pub header_offset: usize,
    /// The member's bytes.
    pub data: Vec<u8>,
}

/// What a symbol index claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedIndex {
    /// `(symbol name, offset of the member header that defines it)`.
    pub entries: Vec<(String, u32)>,
}

/// Read an archive back: the members and the symbol index, if there is one.
///
/// Used by `tests/archive_writer.rs` and by the stage that proves a stale index is still a
/// working archive.
pub fn parse(bytes: &[u8]) -> Result<(Vec<ParsedMember>, Option<ParsedIndex>), String> {
    if bytes.len() < ARCHIVE_MAGIC.len() || &bytes[..8] != ARCHIVE_MAGIC {
        return Err("not an archive: the file does not start with !<arch>\\n".to_string());
    }
    let mut at = ARCHIVE_MAGIC.len();
    let mut members = Vec::new();
    let mut index = None;
    let mut long_names = Vec::<u8>::new();
    while at + MEMBER_HEADER_SIZE <= bytes.len() {
        let header = &bytes[at..at + MEMBER_HEADER_SIZE];
        if &header[58..60] != HEADER_TERMINATOR {
            return Err(format!("member header at {at} does not end with '`\\n'"));
        }
        let raw_name = String::from_utf8_lossy(&header[..16])
            .trim_end()
            .to_string();
        let size: usize = String::from_utf8_lossy(&header[48..58])
            .trim()
            .parse()
            .map_err(|_| format!("member at {at} has an unreadable size field"))?;
        let start = at + MEMBER_HEADER_SIZE;
        let end = start
            .checked_add(size)
            .filter(|e| *e <= bytes.len())
            .ok_or_else(|| format!("member at {at} claims {size} bytes, past the end"))?;
        let data = bytes[start..end].to_vec();
        match raw_name.as_str() {
            "/" => index = Some(parse_index(&data)?),
            "//" => long_names = data,
            _ => {
                let name = if let Some(offset) = raw_name.strip_prefix('/') {
                    let at: usize = offset
                        .parse()
                        .map_err(|_| format!("bad long-name reference '{raw_name}'"))?;
                    let text = String::from_utf8_lossy(&long_names);
                    text.get(at..)
                        .and_then(|s| s.split('/').next())
                        .unwrap_or_default()
                        .to_string()
                } else {
                    raw_name.trim_end_matches('/').to_string()
                };
                members.push(ParsedMember {
                    name,
                    header_offset: at,
                    data,
                });
            }
        }
        at = end + (end % 2);
    }
    Ok((members, index))
}

fn parse_index(data: &[u8]) -> Result<ParsedIndex, String> {
    if data.len() < 4 {
        return Err("the symbol index is shorter than its own count field".to_string());
    }
    let count = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let offsets_end = 4 + 4 * count;
    if data.len() < offsets_end {
        return Err(format!(
            "the symbol index claims {count} symbols but is only {} bytes",
            data.len()
        ));
    }
    let mut entries = Vec::with_capacity(count);
    let mut names = data[offsets_end..].split(|b| *b == 0);
    for i in 0..count {
        let at = 4 + 4 * i;
        let offset = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let name = names
            .next()
            .map(|n| String::from_utf8_lossy(n).to_string())
            .ok_or_else(|| format!("the symbol index has no name for entry {i}"))?;
        entries.push((name, offset));
    }
    Ok(ParsedIndex { entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive() -> Vec<u8> {
        ArchiveBuilder::new()
            .member(Member::new("a.o", vec![1, 2, 3], &["alpha"]))
            .member(Member::new(
                "a_very_long_member_name.o",
                vec![4, 5, 6, 7],
                &["beta", "gamma"],
            ))
            .build()
            .expect("build")
    }

    #[test]
    fn members_round_trip_through_the_parser() {
        let bytes = archive();
        let (members, index) = parse(&bytes).expect("parse");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].name, "a.o");
        assert_eq!(members[0].data, vec![1, 2, 3]);
        assert_eq!(members[1].name, "a_very_long_member_name.o");
        assert_eq!(members[1].data, vec![4, 5, 6, 7]);
        let index = index.expect("the default index mode writes one");
        assert_eq!(index.entries.len(), 3);
    }

    #[test]
    fn the_index_points_at_the_defining_member() {
        let bytes = archive();
        let (members, index) = parse(&bytes).expect("parse");
        let index = index.expect("index");
        for (name, offset) in &index.entries {
            let member = members
                .iter()
                .find(|m| m.header_offset == *offset as usize)
                .unwrap_or_else(|| panic!("no member at offset {offset} for '{name}'"));
            let expected = if name == "alpha" {
                "a.o"
            } else {
                "a_very_long_member_name.o"
            };
            assert_eq!(member.name, expected, "'{name}' points at the wrong member");
        }
    }

    #[test]
    fn a_stale_index_points_everything_at_the_first_member() {
        let bytes = ArchiveBuilder::new()
            .member(Member::new("a.o", vec![1, 2], &["alpha"]))
            .member(Member::new("b.o", vec![3, 4], &["beta"]))
            .index(IndexMode::Stale)
            .build()
            .expect("build");
        let (members, index) = parse(&bytes).expect("parse");
        let index = index.expect("index");
        let first = members[0].header_offset as u32;
        assert!(index.entries.iter().all(|(_, o)| *o == first));
    }

    #[test]
    fn an_omitted_index_still_parses() {
        let bytes = ArchiveBuilder::new()
            .member(Member::new("a.o", vec![1, 2], &["alpha"]))
            .index(IndexMode::Omitted)
            .build()
            .expect("build");
        let (members, index) = parse(&bytes).expect("parse");
        assert_eq!(members.len(), 1);
        assert!(index.is_none());
    }

    #[test]
    fn a_broken_magic_is_rejected() {
        let bytes = ArchiveBuilder::new()
            .member(Member::new("a.o", vec![1, 2], &[]))
            .magic_override(*b"!<arch>!")
            .build()
            .expect("build");
        assert!(parse(&bytes).is_err());
    }
}
