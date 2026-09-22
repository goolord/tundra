//! IFF chunk plumbing for WAV (RIFF, little-endian) and AIFF (FORM, big-endian).
//!
//! WAV tags are written here rather than through lofty so chunks lofty does not
//! model (`smpl`, `cue `, `inst`, ACID, iXML, …) survive byte-for-byte.

use std::ops::Range;
use std::path::Path;

use lofty::config::WriteOptions;
use lofty::id3::v2::Id3v2Tag;
use lofty::tag::TagExt;

use super::read::{WAV_GENRE_KEY, WAV_NATIVE_KEYS, WAV_TITLE_KEY};
use super::write::{TagEdit, apply_id3_edit};
use crate::path_util::path_io_error;

pub(crate) type ChunkId = [u8; 4];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endian {
    Little,
    Big,
}

impl Endian {
    pub(crate) fn read_u32(self, bytes: [u8; 4]) -> u32 {
        match self {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        }
    }

    fn write_u32(self, value: u32) -> [u8; 4] {
        match self {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        }
    }
}

pub(crate) type Chunks = Vec<(ChunkId, Vec<u8>)>;

/// Chunks in `bytes` in order, each as its id and body range. Yields an error
/// for a chunk whose size runs past the end, and nothing after it.
pub(crate) fn scan_chunks(
    bytes: &[u8],
    endian: Endian,
) -> impl Iterator<Item = Result<(ChunkId, Range<usize>), String>> + '_ {
    let mut offset = Some(0usize);
    std::iter::from_fn(move || {
        let at = offset.filter(|at| at + 8 <= bytes.len())?;
        let id: ChunkId = bytes[at..at + 4].try_into().expect("4-byte slice");
        let size = endian.read_u32(bytes[at + 4..at + 8].try_into().expect("4-byte slice")) as usize;
        let start = at + 8;
        let Some(end) = start.checked_add(size).filter(|end| *end <= bytes.len()) else {
            offset = None;
            return Some(Err(format!("Truncated {} chunk", String::from_utf8_lossy(&id))));
        };
        // Odd-sized chunks are followed by a pad byte.
        offset = Some(end + size % 2);
        Some(Ok((id, start..end)))
    })
}

/// The form type and chunks of an IFF file starting with `magic` (`RIFF` or
/// `FORM`). Fails rather than guessing on a size that runs past the end or on
/// trailing data, so a damaged file is never rewritten.
fn parse_form(bytes: &[u8], magic: &[u8; 4], endian: Endian) -> Result<(ChunkId, Chunks), String> {
    if bytes.len() < 12 || &bytes[0..4] != magic {
        return Err(format!("Not a {} file", String::from_utf8_lossy(magic)));
    }
    let form_type: ChunkId = bytes[8..12].try_into().expect("4-byte slice");
    let body = &bytes[12..];
    let ranges: Vec<_> = scan_chunks(body, endian).collect::<Result<_, _>>()?;
    let end = ranges.last().map_or(0, |(_, range)| range.end + range.len() % 2);
    if body.get(end..).is_some_and(|rest| rest.iter().any(|byte| *byte != 0)) {
        return Err("Unexpected trailing bytes after the last chunk".into());
    }
    let chunks = ranges
        .into_iter()
        .map(|(id, range)| (id, body[range].to_vec()))
        .collect();
    Ok((form_type, chunks))
}

fn encode_form(magic: &[u8; 4], form_type: &ChunkId, chunks: &[(ChunkId, Vec<u8>)], endian: Endian) -> Vec<u8> {
    let mut body = form_type.to_vec();
    encode_chunks(chunks, endian, &mut body);
    let mut bytes = Vec::with_capacity(body.len() + 8);
    bytes.extend_from_slice(magic);
    bytes.extend_from_slice(&endian.write_u32(body.len() as u32));
    bytes.extend(body);
    bytes
}

pub(crate) fn parse_riff_wave_chunks(bytes: &[u8]) -> Result<Chunks, String> {
    match parse_form(bytes, b"RIFF", Endian::Little)? {
        (form_type, chunks) if &form_type == b"WAVE" => Ok(chunks),
        _ => Err("Not a RIFF WAVE file".into()),
    }
}

fn encode_chunks(chunks: &[(ChunkId, Vec<u8>)], endian: Endian, out: &mut Vec<u8>) {
    for (id, data) in chunks {
        out.extend_from_slice(id);
        out.extend_from_slice(&endian.write_u32(data.len() as u32));
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
    }
}

pub(crate) fn encode_riff_wave(chunks: &[(ChunkId, Vec<u8>)]) -> Vec<u8> {
    encode_form(b"RIFF", b"WAVE", chunks, Endian::Little)
}

fn is_info_list(id: &ChunkId, data: &[u8]) -> bool {
    id == b"LIST" && data.starts_with(b"INFO")
}

fn is_id3_chunk(id: &ChunkId) -> bool {
    id.eq_ignore_ascii_case(b"id3 ")
}

/// Chunks that only carry tags. Everything else is audio or sampler data that a
/// tag write must leave untouched.
pub(crate) fn is_wav_tag_chunk(id: &ChunkId, data: &[u8]) -> bool {
    is_info_list(id, data) || is_id3_chunk(id)
}

pub(crate) fn is_aiff_tag_chunk(id: &ChunkId) -> bool {
    matches!(id, b"NAME" | b"AUTH" | b"(c) " | b"ANNO" | b"COMT") || is_id3_chunk(id)
}

fn info_field_id(key: &str) -> ChunkId {
    let mut id = [b' '; 4];
    for (slot, byte) in id.iter_mut().zip(key.bytes()) {
        *slot = byte;
    }
    id
}

/// The fields of a `LIST INFO` body, keeping whatever parsed before any damage.
fn parse_info_fields(bytes: &[u8]) -> Chunks {
    scan_chunks(bytes, Endian::Little)
        .map_while(Result::ok)
        .map(|(id, range)| (id, bytes[range].to_vec()))
        .collect()
}

fn set_info_field(fields: &mut Chunks, key: &str, value: &str) {
    let id = info_field_id(key);
    let value = value.trim();
    if value.is_empty() {
        fields.retain(|(found, _)| *found != id);
        return;
    }
    let mut data = value.as_bytes().to_vec();
    data.push(0);
    match fields.iter_mut().find(|(found, _)| *found == id) {
        Some(existing) => existing.1 = data,
        None => fields.push((id, data)),
    }
}

/// Replace `LIST INFO` (and the `id3 ` chunk when BPM/key or an existing ID3
/// tag need it) in the WAV at `path`, keeping every other chunk as-is.
pub(crate) fn write_wav_tags(path: &Path, edit: &TagEdit) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|err| path_io_error("read", path, err))?;
    let mut chunks = parse_riff_wave_chunks(&bytes)?;

    let info_index = chunks.iter().position(|(id, data)| is_info_list(id, data));
    let mut fields = info_index
        .map(|index| parse_info_fields(&chunks[index].1[4..]))
        .unwrap_or_default();
    for (key, value) in edit.keyed(WAV_NATIVE_KEYS, &[WAV_TITLE_KEY, WAV_GENRE_KEY]) {
        set_info_field(&mut fields, key, value);
    }
    let info = (!fields.is_empty()).then(|| {
        let mut body = Vec::from(*b"INFO");
        encode_chunks(&fields, Endian::Little, &mut body);
        (*b"LIST", body)
    });
    replace_chunk(&mut chunks, info_index, info);

    // RIFF INFO has no BPM or key, so those live in an ID3v2 chunk, which is
    // what Mp3tag, foobar2000, and most DAWs read from WAV. Title and genre are
    // mirrored there when a chunk already exists so readers see one value.
    let id3_index = chunks.iter().position(|(id, _)| is_id3_chunk(id));
    let needs_id3 = edit
        .manual
        .is_some_and(|manual| !manual.bpm.trim().is_empty() || !manual.key.trim().is_empty());
    // Instrument-only writes leave an existing ID3 chunk byte-for-byte alone.
    if needs_id3 || (id3_index.is_some() && edit.manual.is_some()) {
        let mut id3 = match id3_index {
            Some(_) => read_wav_id3(&bytes)?,
            None => Id3v2Tag::default(),
        };
        apply_id3_edit(&mut id3, edit);
        let mut data = Vec::new();
        if !id3.is_empty() {
            id3.dump_to(&mut data, WriteOptions::default())
                .map_err(|err| path_io_error("encode ID3 tag for", path, err))?;
        }
        let id3_chunk = (!data.is_empty()).then(|| {
            let id = id3_index.map(|index| chunks[index].0).unwrap_or(*b"id3 ");
            (id, data)
        });
        replace_chunk(&mut chunks, id3_index, id3_chunk);
    }

    std::fs::write(path, encode_riff_wave(&chunks)).map_err(|err| path_io_error("write tags to", path, err))
}

fn replace_chunk(chunks: &mut Chunks, index: Option<usize>, chunk: Option<(ChunkId, Vec<u8>)>) {
    match (index, chunk) {
        (Some(index), Some(chunk)) => chunks[index] = chunk,
        (Some(index), None) => drop(chunks.remove(index)),
        (None, chunk) => chunks.extend(chunk),
    }
}

fn read_wav_id3(bytes: &[u8]) -> Result<Id3v2Tag, String> {
    use lofty::file::AudioFile;
    use lofty::iff::wav::WavFile;

    let options = super::read::write_parse_options();
    WavFile::read_from(&mut std::io::Cursor::new(bytes), options)
        .map_err(|err| format!("Cannot read the existing ID3 chunk: {err}"))?
        .remove_id3v2()
        .ok_or_else(|| "Cannot read the existing ID3 chunk".to_string())
}

/// Move tag chunks ahead of `SSND`. Chunk order is free in AIFF, but some
/// decoders (symphonia included) read sound data to end of file, so tags
/// appended after `SSND` play as a click.
pub(crate) fn move_aiff_tags_before_sound(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|err| path_io_error("read", path, err))?;
    let (form_type, chunks) = parse_form(&bytes, b"FORM", Endian::Big)?;
    let Some(sound) = chunks.iter().position(|(id, _)| id == b"SSND") else {
        return Ok(());
    };
    if !chunks[sound..].iter().any(|(id, _)| is_aiff_tag_chunk(id)) {
        return Ok(());
    }

    let (tags, rest): (Vec<_>, Vec<_>) = chunks.into_iter().partition(|(id, _)| is_aiff_tag_chunk(id));
    let sound = rest.iter().position(|(id, _)| id == b"SSND").expect("SSND kept");
    let ordered: Vec<_> = rest[..sound]
        .iter()
        .chain(&tags)
        .chain(&rest[sound..])
        .cloned()
        .collect();

    std::fs::write(path, encode_form(b"FORM", &form_type, &ordered, Endian::Big))
        .map_err(|err| path_io_error("write tags to", path, err))
}
