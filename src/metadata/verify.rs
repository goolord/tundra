//! Checks a staged tag write before it replaces the original: the audio must be
//! byte-identical, and the fields Tundra set must read back.

use std::hash::Hasher;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use super::read::{Container, generic_tag_fields, read_container_tags_as};
use super::riff::{ChunkId, Endian, is_aiff_tag_chunk, is_wav_tag_chunk, scan_chunks};
use super::write::TagEdit;
use crate::path_util::display_path;

pub(crate) fn verify_staged_write(
    original: &Path,
    staged: &Path,
    container: Container,
    edit: &TagEdit,
) -> Result<(), String> {
    let before = audio_fingerprint(original, container)?;
    let after = audio_fingerprint(staged, container)?;
    if before != after {
        return Err(format!("Refused to save tags to {}: the audio data would have changed", display_path(original)));
    }
    verify_read_back(original, staged, container, edit)
}

fn verify_read_back(original: &Path, staged: &Path, container: Container, edit: &TagEdit) -> Result<(), String> {
    let refuse = |why: &str| format!("Refused to save tags to {}: {why}", display_path(original));
    let tags =
        read_container_tags_as(staged, container).ok_or_else(|| refuse("the tagged copy could not be read back"))?;
    let generic = generic_tag_fields(&tags.generic);
    let native = &tags.native;

    let native_checks = [
        ("instrument", &edit.native.instrument, &native.instrument),
        ("artist", &edit.native.artist, &native.artist),
        ("comment", &edit.native.comment, &native.comment),
    ]
    .into_iter()
    .filter_map(|(label, wanted, found)| Some((label, wanted.as_deref()?, found.as_deref().unwrap_or_default())));
    // Clearing is best-effort: a tag type Tundra does not manage (ID3v1, APE)
    // may still carry an old value.
    let manual_checks = edit
        .manual
        .into_iter()
        .flat_map(|manual| {
            [
                ("title", &manual.title, &generic.title),
                ("genre", &manual.genre, &generic.genre),
                ("BPM", &manual.bpm, &generic.bpm),
                ("key", &manual.key, &generic.key),
            ]
        })
        .filter(|(_, wanted, _)| !wanted.trim().is_empty())
        .map(|(label, wanted, found)| (label, wanted.as_str(), found.as_str()));

    match native_checks.chain(manual_checks).find(|(_, wanted, found)| wanted.trim() != found.trim()) {
        Some((label, ..)) => Err(refuse(&format!("{label} did not read back as written"))),
        None => Ok(()),
    }
}

/// Hash of everything in the file that is not a tag.
fn audio_fingerprint(path: &Path, container: Container) -> Result<u64, String> {
    let fail = |err: String| format!("Cannot verify audio in {}: {err}", display_path(path));
    let file = std::fs::File::open(path).map_err(|err| fail(err.to_string()))?;
    let len = file.metadata().map_err(|err| fail(err.to_string()))?.len();
    let mut reader = BufReader::with_capacity(1 << 16, file);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match container {
        Container::Wav => hash_iff(&mut reader, &mut hasher, Endian::Little, is_wav_tag_chunk),
        Container::Aiff => hash_iff(&mut reader, &mut hasher, Endian::Big, |id, _| is_aiff_tag_chunk(id)),
        Container::Flac => hash_flac(&mut reader, &mut hasher),
        Container::Mp3 => hash_mpeg(&mut reader, &mut hasher, len),
        Container::Ogg => hash_ogg_properties(path, &mut hasher),
    }
    .map_err(fail)?;
    Ok(hasher.finish())
}

fn io_err(err: std::io::Error) -> String {
    err.to_string()
}

/// Hashes the next `len` bytes, or everything left when `len` is `None`.
fn hash_range<R: Read>(reader: &mut R, hasher: &mut impl Hasher, len: Option<u64>) -> Result<(), String> {
    let mut remaining = len.unwrap_or(u64::MAX);
    let mut buf = [0u8; 1 << 16];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let read = reader.read(&mut buf[..want]).map_err(io_err)?;
        if read == 0 {
            return match len {
                Some(_) => Err("Unexpected end of file".into()),
                None => Ok(()),
            };
        }
        hasher.write(&buf[..read]);
        remaining -= read as u64;
    }
    Ok(())
}

/// Hash every chunk that is not a tag. The tag write itself holds the whole
/// file in memory, so reading it in here raises no peak.
fn hash_iff(
    reader: &mut impl Read,
    hasher: &mut impl Hasher,
    endian: Endian,
    is_tag: impl Fn(&ChunkId, &[u8]) -> bool,
) -> Result<(), String> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(io_err)?;
    let body = bytes.get(12..).unwrap_or_default();
    for chunk in scan_chunks(body, endian) {
        let (id, range) = chunk?;
        if !is_tag(&id, &body[range.clone()]) {
            hasher.write(&id);
            hasher.write(&body[range]);
        }
    }
    Ok(())
}

/// STREAMINFO plus every audio frame after the metadata blocks.
fn hash_flac<R: Read + Seek>(reader: &mut R, hasher: &mut impl Hasher) -> Result<(), String> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).map_err(io_err)?;
    if &magic != b"fLaC" {
        return Err("Not a FLAC stream".into());
    }
    loop {
        let mut header = [0u8; 4];
        reader.read_exact(&mut header).map_err(io_err)?;
        let last = header[0] & 0x80 != 0;
        let block_type = header[0] & 0x7F;
        let len = u32::from_be_bytes([0, header[1], header[2], header[3]]) as u64;
        if block_type == 0 {
            hash_range(reader, hasher, Some(len))?;
        } else {
            reader.seek(SeekFrom::Current(len as i64)).map_err(io_err)?;
        }
        if last {
            break;
        }
    }
    hash_range(reader, hasher, None)
}

/// Frames between any leading ID3v2 tags and trailing APE/ID3v1 tags.
fn hash_mpeg<R: Read + Seek>(reader: &mut R, hasher: &mut impl Hasher, len: u64) -> Result<(), String> {
    let mut start = 0u64;
    loop {
        reader.seek(SeekFrom::Start(start)).map_err(io_err)?;
        let mut header = [0u8; 10];
        if reader.read_exact(&mut header).is_err() || &header[..3] != b"ID3" {
            break;
        }
        let size = header[6..10].iter().fold(0u64, |acc, byte| (acc << 7) | u64::from(byte & 0x7F));
        let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
        start += 10 + size + footer;
    }

    let mut end = len;
    if end >= start + 128 {
        reader.seek(SeekFrom::Start(end - 128)).map_err(io_err)?;
        let mut tag = [0u8; 3];
        reader.read_exact(&mut tag).map_err(io_err)?;
        if &tag == b"TAG" {
            end -= 128;
        }
    }
    if end >= start + 32 {
        reader.seek(SeekFrom::Start(end - 32)).map_err(io_err)?;
        let mut footer = [0u8; 32];
        reader.read_exact(&mut footer).map_err(io_err)?;
        if &footer[..8] == b"APETAGEX" {
            let size = u32::from_le_bytes(footer[12..16].try_into().expect("4 bytes")) as u64;
            let flags = u32::from_le_bytes(footer[20..24].try_into().expect("4 bytes"));
            let header = if flags & 0x8000_0000 != 0 { 32 } else { 0 };
            end = end.saturating_sub(size + header).max(start);
        }
    }

    reader.seek(SeekFrom::Start(start)).map_err(io_err)?;
    hash_range(reader, hasher, Some(end.saturating_sub(start)))
}

/// Ogg pages are renumbered when the comment header grows, so bytes cannot be
/// compared directly. Stream shape stands in for them.
fn hash_ogg_properties(path: &Path, hasher: &mut impl Hasher) -> Result<(), String> {
    use lofty::file::AudioFile;

    let mut file = std::fs::File::open(path).map_err(io_err)?;
    let ogg = lofty::ogg::VorbisFile::read_from(&mut file, lofty::config::ParseOptions::new())
        .map_err(|err| err.to_string())?;
    let properties = ogg.properties();
    hasher.write_u128(properties.duration().as_nanos());
    hasher.write_u32(properties.sample_rate());
    hasher.write_u8(properties.channels());
    hasher.write_u32(properties.version());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{ScratchDir, copy_asset};

    #[test]
    fn rejects_a_copy_whose_audio_changed() {
        for (ext, container) in
            [("wav", Container::Wav), ("flac", Container::Flac), ("mp3", Container::Mp3), ("aiff", Container::Aiff)]
        {
            let dir = ScratchDir::new(&format!("verify-{ext}"));
            let original = copy_asset(dir.path(), "tone", ext);
            let staged = dir.path().join(format!("staged.{ext}"));
            let mut bytes = std::fs::read(&original).expect("bytes");
            let index = bytes.len() - 64;
            bytes[index] ^= 0xFF;
            std::fs::write(&staged, bytes).expect("staged");

            let err = verify_staged_write(&original, &staged, container, &TagEdit::default()).expect_err(ext);
            assert!(err.contains("audio data would have changed"), "{ext}: {err}");

            std::fs::copy(&original, &staged).expect("identical copy");
            verify_staged_write(&original, &staged, container, &TagEdit::default())
                .unwrap_or_else(|err| panic!("{ext}: identical copy must pass: {err}"));
        }
    }
}
