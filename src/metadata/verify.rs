//! Checks a staged tag write before it replaces the original: the audio must be
//! byte-identical, and the fields Tundra set must read back.

use std::hash::Hasher;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use super::read::{Container, generic_tag_fields, read_container_tags_as};
use super::riff::{Endian, is_aiff_tag_chunk, is_wav_tag_chunk};
use super::write::TagEdit;

pub(crate) fn verify_staged_write(
    original: &Path,
    staged: &Path,
    container: Container,
    edit: &TagEdit,
) -> Result<(), String> {
    let before = audio_fingerprint(original, container)?;
    let after = audio_fingerprint(staged, container)?;
    if before != after {
        return Err(format!(
            "Refused to save tags to {}: the audio data would have changed",
            original.display()
        ));
    }
    verify_read_back(original, staged, container, edit)
}

fn verify_read_back(original: &Path, staged: &Path, container: Container, edit: &TagEdit) -> Result<(), String> {
    let tags = read_container_tags_as(staged, container).ok_or_else(|| {
        format!(
            "Refused to save tags to {}: the tagged copy could not be read back",
            original.display()
        )
    })?;
    let generic = generic_tag_fields(&tags.generic);
    let native = &tags.native;

    let mut expected: Vec<(&str, &str, String)> = Vec::new();
    for (label, wanted, found) in [
        ("instrument", &edit.native.instrument, &native.instrument),
        ("artist", &edit.native.artist, &native.artist),
        ("comment", &edit.native.comment, &native.comment),
    ] {
        if let Some(wanted) = wanted {
            expected.push((label, wanted.as_str(), found.clone().unwrap_or_default()));
        }
    }
    if let Some(manual) = edit.manual {
        for (label, wanted, found) in [
            ("title", &manual.title, &generic.title),
            ("genre", &manual.genre, &generic.genre),
            ("BPM", &manual.bpm, &generic.bpm),
            ("key", &manual.key, &generic.key),
        ] {
            // Clearing is best-effort: a tag type Tundra does not manage (ID3v1,
            // APE) may still carry an old value.
            if !wanted.trim().is_empty() {
                expected.push((label, wanted.as_str(), found.clone()));
            }
        }
    }

    for (label, wanted, found) in expected {
        if wanted.trim() != found.trim() {
            return Err(format!(
                "Refused to save tags to {}: {label} did not read back as written",
                original.display()
            ));
        }
    }
    Ok(())
}

/// Hash of everything in the file that is not a tag.
fn audio_fingerprint(path: &Path, container: Container) -> Result<u64, String> {
    let fail = |err: String| format!("Cannot verify audio in {}: {err}", path.display());
    let file = std::fs::File::open(path).map_err(|err| fail(err.to_string()))?;
    let len = file.metadata().map_err(|err| fail(err.to_string()))?.len();
    let mut reader = BufReader::with_capacity(1 << 16, file);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match container {
        Container::Wav => hash_iff(&mut reader, &mut hasher, Endian::Little, |id, head| {
            !is_wav_tag_chunk(id, head)
        }),
        Container::Aiff => hash_iff(&mut reader, &mut hasher, Endian::Big, |id, _| !is_aiff_tag_chunk(id)),
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

fn hash_range<R: Read>(reader: &mut R, hasher: &mut impl Hasher, len: u64) -> Result<(), String> {
    let mut remaining = len;
    let mut buf = [0u8; 1 << 16];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        reader.read_exact(&mut buf[..want]).map_err(io_err)?;
        hasher.write(&buf[..want]);
        remaining -= want as u64;
    }
    Ok(())
}

/// Hash every chunk `keep` accepts, streaming bodies instead of loading the
/// file. The first four body bytes are passed to `keep` so `LIST` types can be
/// told apart.
fn hash_iff<R: Read + Seek>(
    reader: &mut R,
    hasher: &mut impl Hasher,
    endian: Endian,
    keep: impl Fn(&[u8; 4], &[u8]) -> bool,
) -> Result<(), String> {
    let end = reader.seek(SeekFrom::End(0)).map_err(io_err)?;
    let mut offset = 12u64;
    while offset + 8 <= end {
        reader.seek(SeekFrom::Start(offset)).map_err(io_err)?;
        let mut header = [0u8; 8];
        reader.read_exact(&mut header).map_err(io_err)?;
        let id: [u8; 4] = header[..4].try_into().expect("4-byte slice");
        let size = u64::from(endian.read_u32(header[4..].try_into().expect("4-byte slice")));
        let body_start = offset + 8;
        if body_start + size > end {
            return Err(format!("Truncated {} chunk", String::from_utf8_lossy(&id)));
        }
        let head_len = size.min(4) as usize;
        let mut head = [0u8; 4];
        reader.read_exact(&mut head[..head_len]).map_err(io_err)?;
        if keep(&id, &head[..head_len]) {
            hasher.write(&id);
            hasher.write(&head[..head_len]);
            hash_range(reader, hasher, size - head_len as u64)?;
        }
        offset = body_start + size + size % 2;
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
            hash_range(reader, hasher, len)?;
        } else {
            reader.seek(SeekFrom::Current(len as i64)).map_err(io_err)?;
        }
        if last {
            break;
        }
    }
    std::io::copy(reader, &mut HashWriter(hasher)).map_err(io_err)?;
    Ok(())
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
        let size = header[6..10]
            .iter()
            .fold(0u64, |acc, byte| (acc << 7) | u64::from(byte & 0x7F));
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
    hash_range(reader, hasher, end.saturating_sub(start))
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

struct HashWriter<'a, H: Hasher>(&'a mut H);

impl<H: Hasher> std::io::Write for HashWriter<'_, H> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::ScratchDir;

    #[test]
    fn rejects_a_copy_whose_audio_changed() {
        for (ext, container) in [
            ("wav", Container::Wav),
            ("flac", Container::Flac),
            ("mp3", Container::Mp3),
            ("aiff", Container::Aiff),
        ] {
            let dir = ScratchDir::new(&format!("verify-{ext}"));
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets")
                .join(format!("tone.{ext}"));
            let original = dir.path().join(format!("tone.{ext}"));
            let staged = dir.path().join(format!("staged.{ext}"));
            std::fs::copy(&fixture, &original).expect("original");
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
