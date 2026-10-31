//! Read-only access to Chromium's `Local Storage/leveldb`, where the Electron app kept its
//! renderer preferences. Only the newest value of each requested key is recovered; nothing is
//! ever written back.

use std::{collections::HashMap, path::Path};

const BLOCK_SIZE: usize = 32 * 1024;
const TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;

/// The newest stored string for each of `keys` under `origin`, such as `file://`. A key whose
/// newest record is a deletion is absent.
pub fn read(
    directory: &Path,
    origin: &str,
    keys: &[&str],
) -> Result<HashMap<String, String>, String> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        format!(
            "Cannot list {}: {error}. Check that the folder is readable.",
            directory.display()
        )
    })?;
    let wanted: HashMap<Vec<u8>, &str> = keys
        .iter()
        .flat_map(|key| {
            [
                storage_key(origin, key, false),
                storage_key(origin, key, true),
            ]
            .map(|bytes| (bytes, *key))
        })
        .collect();
    let mut newest: HashMap<&str, (u64, Option<Vec<u8>>)> = HashMap::new();
    let mut record = |key: &[u8], sequence: u64, value: Option<&[u8]>| {
        if let Some(name) = wanted.get(key)
            && newest.get(name).is_none_or(|(known, _)| *known <= sequence)
        {
            newest.insert(name, (sequence, value.map(<[u8]>::to_vec)));
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let extension = path.extension().and_then(|extension| extension.to_str());
        if !matches!(extension, Some("log" | "ldb" | "sst")) {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            format!(
                "Cannot read {}: {error}. Close the Electron app and retry.",
                path.display()
            )
        })?;
        let parsed = if extension == Some("log") {
            read_log(&bytes, &mut record)
        } else {
            read_table(&bytes, &mut record)
        };
        parsed.map_err(|error| {
            format!(
                "Cannot read {}: {error}. The stored preferences may be damaged; set them again in Preferences.",
                path.display()
            )
        })?;
    }
    Ok(newest
        .into_iter()
        .filter_map(|(key, (_, value))| Some((key.to_owned(), decode_value(&value?)?)))
        .collect())
}

/// Chromium's key layout: `_` + origin + NUL + an encoding byte (1 for Latin-1, 0 for UTF-16LE)
/// + the script-visible key.
pub fn storage_key(origin: &str, key: &str, utf16: bool) -> Vec<u8> {
    let mut bytes = format!("_{origin}\0").into_bytes();
    if utf16 {
        bytes.push(0);
        bytes.extend(key.encode_utf16().flat_map(u16::to_le_bytes));
    } else {
        bytes.push(1);
        bytes.extend(key.chars().map(|char| char as u8));
    }
    bytes
}

/// Chromium's value layout: an encoding byte followed by Latin-1 or UTF-16LE text.
pub fn decode_value(bytes: &[u8]) -> Option<String> {
    match bytes.split_first()? {
        (1, text) => Some(text.iter().map(|&byte| byte as char).collect()),
        (0, text) if text.len() % 2 == 0 => String::from_utf16(
            &text
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect::<Vec<_>>(),
        )
        .ok(),
        _ => None,
    }
}

type Record<'a> = dyn FnMut(&[u8], u64, Option<&[u8]>) + 'a;

fn read_log(bytes: &[u8], record: &mut Record) -> Result<(), String> {
    let mut batch = Vec::new();
    for block in bytes.chunks(BLOCK_SIZE) {
        let mut offset = 0;
        while offset + 7 <= block.len() {
            let length = u16::from_le_bytes([block[offset + 4], block[offset + 5]]) as usize;
            let kind = block[offset + 6];
            let start = offset + 7;
            let Some(data) = block.get(start..start + length) else {
                return Ok(());
            };
            offset = start + length;
            match kind {
                0 => break,
                1 => read_batch(data, record)?,
                2 => batch = data.to_vec(),
                3 => batch.extend_from_slice(data),
                4 => {
                    batch.extend_from_slice(data);
                    read_batch(&std::mem::take(&mut batch), record)?;
                }
                _ => return Err(format!("unknown log record type {kind}")),
            }
        }
    }
    Ok(())
}

fn read_batch(bytes: &[u8], record: &mut Record) -> Result<(), String> {
    let mut reader = Reader(bytes);
    let sequence = reader.fixed64()?;
    let count = reader.fixed32()?;
    for index in 0..u64::from(count) {
        let tag = reader.byte()?;
        let key = reader.slice()?;
        match tag {
            1 => record(key, sequence + index, Some(reader.slice()?)),
            0 => record(key, sequence + index, None),
            _ => return Err(format!("unknown batch tag {tag}")),
        }
    }
    Ok(())
}

fn read_table(bytes: &[u8], record: &mut Record) -> Result<(), String> {
    let footer = bytes
        .len()
        .checked_sub(48)
        .map(|start| &bytes[start..])
        .ok_or("table is shorter than its footer")?;
    if u64::from_le_bytes(footer[40..48].try_into().map_err(|_| "short footer")?) != TABLE_MAGIC {
        return Err("table footer has the wrong magic number".into());
    }
    let mut handles = Reader(footer);
    handles.handle()?;
    let index = block(bytes, handles.handle()?)?;
    for (_, handle) in entries(&index)? {
        for (key, value) in entries(&block(bytes, Reader(&handle).handle()?)?)? {
            let split = key
                .len()
                .checked_sub(8)
                .ok_or("internal key is shorter than its trailer")?;
            let trailer = u64::from_le_bytes(key[split..].try_into().map_err(|_| "short key")?);
            let value = (trailer & 0xff == 1).then_some(value.as_slice());
            record(&key[..split], trailer >> 8, value);
        }
    }
    Ok(())
}

fn block(bytes: &[u8], (offset, size): (usize, usize)) -> Result<Vec<u8>, String> {
    let data = bytes
        .get(offset..offset + size + 5)
        .ok_or("block handle points past the end of the table")?;
    match data[size] {
        0 => Ok(data[..size].to_vec()),
        1 => snappy(&data[..size]),
        kind => Err(format!("unknown block compression {kind}")),
    }
}

type Entry = (Vec<u8>, Vec<u8>);

fn entries(block: &[u8]) -> Result<Vec<Entry>, String> {
    let restarts = block
        .len()
        .checked_sub(4)
        .map(|start| u32::from_le_bytes(block[start..].try_into().unwrap_or_default()) as usize)
        .ok_or("block is shorter than its restart count")?;
    let end = block
        .len()
        .checked_sub(4 + restarts * 4)
        .ok_or("block restart array is larger than the block")?;
    let mut reader = Reader(&block[..end]);
    let mut key = Vec::new();
    let mut result = Vec::new();
    while !reader.0.is_empty() {
        let shared = reader.varint()? as usize;
        let unshared = reader.varint()? as usize;
        let length = reader.varint()? as usize;
        key.truncate(shared);
        key.extend_from_slice(reader.take(unshared)?);
        result.push((key.clone(), reader.take(length)?.to_vec()));
    }
    Ok(result)
}

/// Decompresses one raw Snappy block.
pub fn snappy(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = Reader(input);
    let length = reader.varint()? as usize;
    let mut output = Vec::with_capacity(length);
    while !reader.0.is_empty() {
        let tag = reader.byte()?;
        let (length, offset) = match tag & 3 {
            0 => {
                let short = (tag >> 2) as usize;
                let length = if short < 60 {
                    short + 1
                } else {
                    let bytes = reader.take(short - 59)?;
                    bytes
                        .iter()
                        .rev()
                        .fold(0usize, |total, &byte| total << 8 | byte as usize)
                        + 1
                };
                output.extend_from_slice(reader.take(length)?);
                continue;
            }
            1 => (
                ((tag >> 2) & 7) as usize + 4,
                ((tag as usize >> 5) << 8) | reader.byte()? as usize,
            ),
            2 => (
                (tag >> 2) as usize + 1,
                u16::from_le_bytes(reader.take(2)?.try_into().map_err(|_| "short copy")?) as usize,
            ),
            _ => (
                (tag >> 2) as usize + 1,
                u32::from_le_bytes(reader.take(4)?.try_into().map_err(|_| "short copy")?) as usize,
            ),
        };
        let start = output
            .len()
            .checked_sub(offset)
            .filter(|_| offset > 0)
            .ok_or("snappy copy reaches before the start of the block")?;
        for index in start..start + length {
            let byte = output[index];
            output.push(byte);
        }
    }
    if output.len() != length {
        return Err("snappy block length does not match its header".into());
    }
    Ok(output)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if count > self.0.len() {
            return Err("record ends early".into());
        }
        let (head, tail) = self.0.split_at(count);
        self.0 = tail;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn fixed32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| "short integer")?,
        ))
    }

    fn fixed64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| "short integer")?,
        ))
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("varint is too long".into())
    }

    fn slice(&mut self) -> Result<&'a [u8], String> {
        let length = self.varint()? as usize;
        self.take(length)
    }

    fn handle(&mut self) -> Result<(usize, usize), String> {
        Ok((self.varint()? as usize, self.varint()? as usize))
    }
}
