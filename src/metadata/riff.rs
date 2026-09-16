//! IFF chunk plumbing for WAV (RIFF, little-endian) and AIFF (FORM, big-endian).
//!
//! WAV tags are written here rather than through lofty so chunks lofty does not
//! model (`smpl`, `cue `, `inst`, ACID, iXML, …) survive byte-for-byte.

use std::ops::Range;

use lofty::config::WriteOptions;
use lofty::id3::v2::Id3v2Tag;
use lofty::tag::TagExt;

use super::read::{WAV_ARTIST_KEY, WAV_COMMENT_KEY, WAV_GENRE_KEY, WAV_INSTRUMENT_KEY, WAV_TITLE_KEY};
use super::write::{apply_id3_edit, TagEdit};

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

/// Chunk ids and byte ranges (of each chunk body) inside `bytes`, which start
/// right after a 12-byte form header. Fails rather than guessing on a size that
/// runs past the end, so a damaged file is never rewritten.
pub(crate) fn chunk_ranges(bytes: &[u8], endian: Endian) -> Result<Vec<(ChunkId, Range<usize>)>, String> {
    let mut offset = 0usize;
    let mut chunks = Vec::new();
    while offset + 8 <= bytes.len() {
        let id: ChunkId = bytes[offset..offset + 4].try_into().expect("4-byte slice");
        let size = endian.read_u32(bytes[offset + 4..offset + 8].try_into().expect("4-byte slice"))
            as usize;
        let start = offset + 8;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| format!("Truncated {} chunk", String::from_utf8_lossy(&id)))?;
        chunks.push((id, start..end));
        offset = end + size % 2;
    }
    if offset < bytes.len() && bytes[offset..].iter().any(|byte| *byte != 0) {
        return Err("Unexpected trailing bytes after the last chunk".into());
    }
    Ok(chunks)
}

pub(crate) fn parse_riff_wave_chunks(bytes: &[u8]) -> Result<Vec<(ChunkId, Vec<u8>)>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Not a RIFF WAVE file".into());
    }
    let body = &bytes[12..];
    Ok(chunk_ranges(body, Endian::Little)?
        .into_iter()
        .map(|(id, range)| (id, body[range].to_vec()))
        .collect())
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
    let mut body = Vec::from(*b"WAVE");
    encode_chunks(chunks, Endian::Little, &mut body);
    let mut bytes = Vec::with_capacity(body.len() + 8);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend(body);
    bytes
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

type InfoFields = Vec<(ChunkId, Vec<u8>)>;

fn parse_info_fields(bytes: &[u8]) -> InfoFields {
    let mut offset = 0usize;
    let mut fields = Vec::new();
    while offset + 8 <= bytes.len() {
        let id: ChunkId = bytes[offset..offset + 4].try_into().expect("4-byte slice");
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().expect("4-byte slice"))
            as usize;
        let start = offset + 8;
        let Some(end) = start.checked_add(size).filter(|end| *end <= bytes.len()) else {
            break;
        };
        fields.push((id, bytes[start..end].to_vec()));
        offset = end + size % 2;
    }
    fields
}

fn set_info_field(fields: &mut InfoFields, key: &str, value: &str) {
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

fn apply_info_edit(fields: &mut InfoFields, edit: &TagEdit) {
    for (key, value) in [
        (WAV_INSTRUMENT_KEY, &edit.native.instrument),
        (WAV_ARTIST_KEY, &edit.native.artist),
        (WAV_COMMENT_KEY, &edit.native.comment),
    ] {
        if let Some(value) = value {
            set_info_field(fields, key, value);
        }
    }
    if let Some(manual) = edit.manual {
        set_info_field(fields, WAV_TITLE_KEY, &manual.title);
        set_info_field(fields, WAV_GENRE_KEY, &manual.genre);
    }
}

/// Replace `LIST INFO` (and the `id3 ` chunk when BPM/key or an existing ID3
/// tag need it) in the WAV at `path`, keeping every other chunk as-is.
pub(crate) fn write_wav_tags(path: &std::path::Path, edit: &TagEdit) -> Result<(), String> {
    use crate::path_util::path_io_error;

    let bytes = std::fs::read(path).map_err(|err| path_io_error("read", path, err))?;
    let mut chunks = parse_riff_wave_chunks(&bytes)?;

    let info_index = chunks.iter().position(|(id, data)| is_info_list(id, data));
    let mut fields = info_index
        .map(|index| parse_info_fields(&chunks[index].1[4..]))
        .unwrap_or_default();
    apply_info_edit(&mut fields, edit);
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

    std::fs::write(path, encode_riff_wave(&chunks))
        .map_err(|err| path_io_error("write tags to", path, err))
}

fn replace_chunk(
    chunks: &mut Vec<(ChunkId, Vec<u8>)>,
    index: Option<usize>,
    chunk: Option<(ChunkId, Vec<u8>)>,
) {
    match (index, chunk) {
        (Some(index), Some(chunk)) => chunks[index] = chunk,
        (Some(index), None) => {
            chunks.remove(index);
        }
        (None, Some(chunk)) => chunks.push(chunk),
        (None, None) => {}
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
pub(crate) fn move_aiff_tags_before_sound(path: &std::path::Path) -> Result<(), String> {
    use crate::path_util::path_io_error;

    let bytes = std::fs::read(path).map_err(|err| path_io_error("read", path, err))?;
    if bytes.len() < 12 || &bytes[0..4] != b"FORM" {
        return Err("Not an AIFF file".into());
    }
    let body = &bytes[12..];
    let chunks: Vec<(ChunkId, Vec<u8>)> = chunk_ranges(body, Endian::Big)?
        .into_iter()
        .map(|(id, range)| (id, body[range].to_vec()))
        .collect();
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

    let mut form = bytes[8..12].to_vec();
    encode_chunks(&ordered, Endian::Big, &mut form);
    let mut out = Vec::with_capacity(form.len() + 8);
    out.extend_from_slice(b"FORM");
    out.extend_from_slice(&(form.len() as u32).to_be_bytes());
    out.extend(form);
    std::fs::write(path, out).map_err(|err| path_io_error("write tags to", path, err))
}
