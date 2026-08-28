use lofty::ogg::tag::VorbisComments;
use std::path::Path;

use crate::path_util::{open_file, path_io_error};
use crate::types::is_audio;

use super::auto_tag::AutoTagFieldStatus;
use super::fields::ManualTagEdits;
use super::hints::artist_hint_from_path;
use super::read::{
    apply_native_tags_staged, apply_vorbis_tags, durable_instrument, non_empty, read_native_tags,
    stage_and_replace, tundra_comment, write_native_tags, write_parse_options,
    write_wav_info_preserving_chunks, Container, NativeTags, TUNDRA_TAG_VERSION,
};

fn apply_manual_generic_edits(tag: &mut lofty::tag::Tag, edits: &ManualTagEdits) {
    use lofty::tag::Accessor;
    use lofty::tag::ItemKey;

    let set_or_clear = |tag: &mut lofty::tag::Tag, key: ItemKey, value: &str| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            tag.remove_key(key);
        } else {
            tag.insert_text(key, trimmed.to_string());
        }
    };

    let title = edits.title.trim();
    if title.is_empty() {
        tag.remove_title();
    } else {
        tag.set_title(title.to_string());
    }

    let genre = edits.genre.trim();
    if genre.is_empty() {
        tag.remove_genre();
    } else {
        tag.set_genre(genre.to_string());
    }

    set_or_clear(tag, ItemKey::IntegerBpm, &edits.bpm);
    set_or_clear(tag, ItemKey::Bpm, &edits.bpm);
    set_or_clear(tag, ItemKey::InitialKey, &edits.key);
}

fn save_generic_edits_lofty(
    tagged: &mut lofty::file::TaggedFile,
    staged: &Path,
    edits: &ManualTagEdits,
) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::tag::Tag;

    let tag_type = tagged.primary_tag_type();

    if tagged.tag_mut(tag_type).is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged
        .tag_mut(tag_type)
        .ok_or_else(|| format!("Failed to open tags for {}", staged.display()))?;
    apply_manual_generic_edits(tag, edits);

    tagged
        .save_to_path(staged, WriteOptions::default())
        .map_err(|err| path_io_error("write tags to", staged, err))
}

fn apply_generic_tags_staged(staged: &Path, edits: &ManualTagEdits) -> Result<(), String> {
    use lofty::probe::Probe;

    if let Ok(mut tagged) = Probe::open(staged).and_then(|probe| {
        probe.options(write_parse_options()).read()
    }) {
        return save_generic_edits_lofty(&mut tagged, staged, edits);
    }

    match Container::of(staged) {
        Some(Container::Wav) => write_wav_info_preserving_chunks(staged, None, Some(edits)),
        Some(Container::Flac) => {
            apply_manual_vorbis_staged(staged, &NativeTags::default(), edits, Container::Flac)
        }
        Some(Container::Ogg) => {
            apply_manual_vorbis_staged(staged, &NativeTags::default(), edits, Container::Ogg)
        }
        Some(Container::Mp3) => apply_generic_tags_mp3_staged(staged, edits),
        Some(Container::Aiff) => apply_generic_tags_aiff_staged(staged, edits),
        None => Err(format!(
            "Failed to read tags from {}: file format is not supported for generic tag edits",
            staged.display()
        )),
    }
}

fn set_or_remove_vorbis_key(vorbis: &mut VorbisComments, key: &str, value: &str) {
    let trimmed = value.trim();
    let _removed: Vec<_> = vorbis.remove(key).collect();
    if !trimmed.is_empty() {
        vorbis.insert(key.to_string(), trimmed.to_string());
    }
}

fn apply_manual_generic_vorbis(vorbis: &mut VorbisComments, edits: &ManualTagEdits) {
    set_or_remove_vorbis_key(vorbis, "TITLE", &edits.title);
    set_or_remove_vorbis_key(vorbis, "GENRE", &edits.genre);
    set_or_remove_vorbis_key(vorbis, "BPM", &edits.bpm);
    set_or_remove_vorbis_key(vorbis, "INITIALKEY", &edits.key);
}

fn apply_manual_vorbis_staged(
    staged: &Path,
    native: &NativeTags,
    edits: &ManualTagEdits,
    container: Container,
) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::file::AudioFile;

    let options = write_parse_options();
    let mut file = open_file(staged)?;

    match container {
        Container::Flac => {
            use lofty::flac::FlacFile;
            let mut flac = FlacFile::read_from(&mut file, options)
                .map_err(|err| path_io_error("read", staged, err))?;
            let mut vorbis = flac.remove_vorbis_comments().unwrap_or_default();
            if !native.is_empty() {
                apply_vorbis_tags(&mut vorbis, native);
            }
            apply_manual_generic_vorbis(&mut vorbis, edits);
            flac.set_vorbis_comments(vorbis);
            flac.save_to_path(staged, WriteOptions::default())
        }
        Container::Ogg => {
            use lofty::ogg::VorbisFile;
            let mut ogg = VorbisFile::read_from(&mut file, options)
                .map_err(|err| path_io_error("read", staged, err))?;
            let mut vorbis = std::mem::take(ogg.vorbis_comments_mut());
            if !native.is_empty() {
                apply_vorbis_tags(&mut vorbis, native);
            }
            apply_manual_generic_vorbis(&mut vorbis, edits);
            *ogg.vorbis_comments_mut() = vorbis;
            ogg.save_to_path(staged, WriteOptions::default())
        }
        _ => unreachable!("vorbis staged write only applies to FLAC/OGG"),
    }
    .map_err(|err| path_io_error("write tags to", staged, err))
}

fn apply_generic_tags_mp3_staged(staged: &Path, edits: &ManualTagEdits) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::file::AudioFile;
    use lofty::mpeg::MpegFile;
    use lofty::tag::Tag;

    let options = write_parse_options();
    let mut file = open_file(staged)?;
    let mut mp3 = MpegFile::read_from(&mut file, options)
        .map_err(|err| path_io_error("read", staged, err))?;
    let id3 = mp3.remove_id3v2().unwrap_or_default();
    let mut tag = Tag::from(id3);
    apply_manual_generic_edits(&mut tag, edits);
    mp3.set_id3v2(tag.into());
    mp3.save_to_path(staged, WriteOptions::default())
        .map_err(|err| path_io_error("write tags to", staged, err))
}

fn apply_generic_tags_aiff_staged(staged: &Path, edits: &ManualTagEdits) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::file::AudioFile;
    use lofty::iff::aiff::AiffFile;
    use lofty::tag::Tag;

    let options = write_parse_options();
    let mut file = open_file(staged)?;
    let mut aiff = AiffFile::read_from(&mut file, options)
        .map_err(|err| path_io_error("read", staged, err))?;
    let text = aiff.remove_text_chunks().unwrap_or_default();
    let mut tag = Tag::from(text);
    apply_manual_generic_edits(&mut tag, edits);
    aiff.set_text_chunks(lofty::iff::aiff::AiffTextChunks::from(tag));
    aiff.save_to_path(staged, WriteOptions::default())
        .map_err(|err| path_io_error("write tags to", staged, err))
}

fn sidecar_fields_for_fallback(path: &Path, edits: &ManualTagEdits) -> ManualTagEdits {
    let existing = crate::tag_store::manual_fields(path).unwrap_or_default();
    ManualTagEdits {
        instrument: non_empty(Some(edits.instrument.as_str())).unwrap_or(existing.instrument),
        artist: non_empty(Some(edits.artist.as_str())).unwrap_or(existing.artist),
        comment: non_empty(Some(edits.comment.as_str())).unwrap_or(existing.comment),
        title: edits.title.trim().to_string(),
        bpm: edits.bpm.trim().to_string(),
        key: edits.key.trim().to_string(),
        genre: edits.genre.trim().to_string(),
    }
}

fn save_manual_edits_to_sidecar(
    path: &Path,
    edits: &ManualTagEdits,
    disk_err: String,
) -> Result<(), String> {
    crate::tag_store::set_manual_fields(
        path,
        &sidecar_fields_for_fallback(path, edits),
        TUNDRA_TAG_VERSION,
    )
    .map_err(|store_err| format!("{disk_err} (sidecar: {store_err})"))
}

fn apply_manual_disk_tags(
    staged: &Path,
    native: &NativeTags,
    edits: &ManualTagEdits,
) -> Result<(), String> {
    match Container::of(staged) {
        Some(Container::Wav) => {
            let native_ref = (!native.is_empty()).then_some(native);
            return write_wav_info_preserving_chunks(staged, native_ref, Some(edits));
        }
        Some(Container::Flac) => {
            return apply_manual_vorbis_staged(staged, native, edits, Container::Flac);
        }
        Some(Container::Ogg) => {
            return apply_manual_vorbis_staged(staged, native, edits, Container::Ogg);
        }
        _ => {}
    }
    if !native.is_empty() {
        apply_native_tags_staged(staged, native)?;
    }
    apply_generic_tags_staged(staged, edits)
}

fn require_audio(path: &Path) -> Result<(), String> {
    if is_audio(path) {
        Ok(())
    } else {
        Err(format!("Not an audio file: {}", path.display()))
    }
}

pub fn write_manual_tags(path: &Path, edits: &ManualTagEdits) -> Result<(), String> {
    require_audio(path)?;

    let native = NativeTags {
        instrument: non_empty(Some(edits.instrument.as_str())),
        artist: non_empty(Some(edits.artist.as_str())),
        comment: non_empty(Some(edits.comment.as_str())),
    };

    let try_disk = |native: &NativeTags| {
        stage_and_replace(path, |staged| apply_manual_disk_tags(staged, native, edits))
    };

    match try_disk(&native) {
        Ok(()) => {
            let _ = crate::tag_store::clear_manual_fields(path);
            Ok(())
        }
        Err(err) if native.instrument.is_some() => {
            crate::tag_store::set_instrument(
                path,
                native.instrument.as_ref().expect("checked above"),
                TUNDRA_TAG_VERSION,
            )
            .map_err(|store_error| format!("{err} (sidecar: {store_error})"))?;
            let native_rest = NativeTags {
                instrument: None,
                artist: native.artist,
                comment: native.comment,
            };
            match try_disk(&native_rest) {
                Ok(()) => {
                    let _ = crate::tag_store::clear_manual_fields(path);
                    Ok(())
                }
                Err(retry_err) => save_manual_edits_to_sidecar(path, edits, retry_err),
            }
        }
        Err(err) => save_manual_edits_to_sidecar(path, edits, err),
    }
}

/// Native container tags travel with the file. When the container rejects the
/// write the label is recorded in the sidecar store instead, so search still
/// finds the file.
pub fn write_auto_tags(path: &Path, instrument: &str) -> Result<bool, String> {
    require_audio(path)?;
    let native = read_native_tags(path);
    let native_writable = native.is_some();
    let native = native.unwrap_or_default();
    let durable = durable_instrument(path, &native, None);
    let comment = native.comment.as_deref().unwrap_or_default();
    let status = AutoTagFieldStatus::from_parts(
        path,
        durable.as_deref().unwrap_or_default(),
        native.instrument.as_deref().unwrap_or_default(),
        native.artist.as_deref().unwrap_or_default(),
        comment,
        native_writable,
    );
    if status.is_complete() {
        return Ok(false);
    }

    let instrument = instrument.trim();
    if status.allows_instrument_work() && instrument.is_empty() {
        return Err("Instrument label cannot be empty".into());
    }

    let current = durable.as_deref().unwrap_or_default().trim();
    let instrument_changed = status.allows_instrument_work()
        && !instrument.is_empty()
        && !current.eq_ignore_ascii_case(instrument);
    let pending = NativeTags {
        instrument: instrument_changed.then(|| instrument.to_string()),
        artist: status
            .needs_artist
            .then(|| artist_hint_from_path(path))
            .flatten()
            .filter(|artist| !artist.is_empty()),
        comment: status
            .needs_comment
            .then(|| tundra_comment(native.comment.as_deref())),
    };

    if pending.is_empty() {
        return Ok(false);
    }

    match write_native_tags(path, &pending) {
        Ok(()) => Ok(true),
        Err(native_error) => {
            // Container refused the write (unsupported layout, unwritable file).
            // Record the label in the sidecar store so search still finds it.
            // Never stamp a sidecar over a native instrument: durable reads
            // prefer native, and a sidecar vN would skip retag forever.
            if let Some(instrument) = &pending.instrument {
                if native.instrument.as_ref().is_none_or(|value| value.trim().is_empty()) {
                    crate::tag_store::set_instrument(path, instrument, TUNDRA_TAG_VERSION)
                        .map_err(|store_error| format!("{native_error} (sidecar: {store_error})"))?;
                    return Ok(true);
                }
                return Err(native_error);
            }
            // The instrument is already covered, and the remaining fields have
            // nowhere to go in a container this broken. Report the file as done
            // rather than failing it again on every future scan.
            if durable.is_some() && !status.can_retag_instrument {
                return Ok(false);
            }
            Err(native_error)
        }
    }
}
