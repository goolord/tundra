//! Every change Tundra makes to an audio file goes through [`write_tags`]:
//!
//! 1. copy the file to a same-directory temp,
//! 2. edit the copy with the container's own tag type (never lofty's lossy
//!    generic `Tag`, which drops frames it does not model),
//! 3. prove the audio payload is byte-identical and the new tags read back,
//! 4. refuse if the original changed on disk meanwhile,
//! 5. atomically swap the copy in.
//!
//! A failure at any step leaves the original untouched and removes the temp.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::file::AudioFile;
use lofty::id3::v2::{Frame, FrameId, Id3v2Tag, TextInformationFrame};
use lofty::ogg::tag::VorbisComments;
use lofty::tag::Accessor;
use lofty::TextEncoding;

use crate::path_util::{open_file, path_io_error, FileStamp};
use super::read::is_audio;

use super::auto_tag::{inspect_native, NativeInspection};
use super::fields::ManualTagEdits;
use super::hints::artist_hint_from_path;
use super::read::{
    non_empty, tundra_comment, write_parse_options,
    Container, NativeTags, ID3_INSTRUMENT_KEY, TUNDRA_TAG_VERSION, VORBIS_ARTIST_KEY,
    VORBIS_COMMENT_KEY, VORBIS_INSTRUMENT_KEY,
};

const VORBIS_TITLE_KEY: &str = "TITLE";
const VORBIS_GENRE_KEY: &str = "GENRE";
const VORBIS_BPM_KEY: &str = "BPM";
const VORBIS_KEY_KEY: &str = "INITIALKEY";

/// One tag write.
///
/// `native` fields are set when `Some` and left alone when `None`. `manual`
/// fields (title, genre, BPM, key) are set when non-empty and cleared when empty,
/// matching what the tag editor shows.
#[derive(Debug, Default)]
pub(crate) struct TagEdit<'a> {
    pub native: NativeTags,
    pub manual: Option<&'a ManualTagEdits>,
}

impl TagEdit<'_> {
    fn is_empty(&self) -> bool {
        self.native.is_empty() && self.manual.is_none()
    }
}

fn set_vorbis(vorbis: &mut VorbisComments, key: &str, value: &str) {
    let _removed: Vec<_> = vorbis.remove(key).collect();
    let value = value.trim();
    if !value.is_empty() {
        vorbis.insert(key.to_string(), value.to_string());
    }
}

fn apply_vorbis_edit(vorbis: &mut VorbisComments, edit: &TagEdit) {
    for (key, value) in [
        (VORBIS_INSTRUMENT_KEY, &edit.native.instrument),
        (VORBIS_ARTIST_KEY, &edit.native.artist),
        (VORBIS_COMMENT_KEY, &edit.native.comment),
    ] {
        if let Some(value) = value {
            set_vorbis(vorbis, key, value);
        }
    }
    if let Some(manual) = edit.manual {
        set_vorbis(vorbis, VORBIS_TITLE_KEY, &manual.title);
        set_vorbis(vorbis, VORBIS_GENRE_KEY, &manual.genre);
        set_vorbis(vorbis, VORBIS_BPM_KEY, &manual.bpm);
        set_vorbis(vorbis, VORBIS_KEY_KEY, &manual.key);
    }
}

fn set_id3_text(id3: &mut Id3v2Tag, id: &'static str, value: &str) {
    let frame_id = FrameId::Valid(Cow::Borrowed(id));
    drop(id3.remove(&frame_id));
    let value = value.trim();
    if !value.is_empty() {
        id3.insert(Frame::Text(TextInformationFrame::new(
            frame_id,
            TextEncoding::UTF8,
            value.to_string(),
        )));
    }
}

pub(crate) fn apply_id3_edit(id3: &mut Id3v2Tag, edit: &TagEdit) {
    if let Some(instrument) = &edit.native.instrument {
        id3.insert_user_text(ID3_INSTRUMENT_KEY.to_string(), instrument.trim().to_string());
    }
    if let Some(artist) = &edit.native.artist {
        id3.set_artist(artist.trim().to_string());
    }
    if let Some(comment) = &edit.native.comment {
        id3.set_comment(comment.trim().to_string());
    }
    if let Some(manual) = edit.manual {
        set_id3_text(id3, "TIT2", &manual.title);
        set_id3_text(id3, "TCON", &manual.genre);
        set_id3_text(id3, "TBPM", &manual.bpm);
        set_id3_text(id3, "TKEY", &manual.key);
    }
}

fn apply_aiff_text_edit(text: &mut lofty::iff::aiff::AiffTextChunks, edit: &TagEdit) {
    if let Some(artist) = &edit.native.artist {
        text.set_artist(artist.trim().to_string());
    }
    if let Some(comment) = &edit.native.comment {
        text.set_comment(comment.trim().to_string());
    }
    if let Some(manual) = edit.manual {
        match manual.title.trim() {
            "" => text.remove_title(),
            title => text.set_title(title.to_string()),
        }
    }
}

/// Apply `edit` to the staged copy, in place.
fn apply_tag_edit(staged: &Path, container: Container, edit: &TagEdit) -> Result<(), String> {
    let options = write_parse_options();
    let read_error = |err: lofty::error::FileParseError| path_io_error("read", staged, err);
    // Each parser reads from a handle that is closed again before saving over the file.
    let open = || open_file(staged);
    let saved = match container {
        Container::Wav => return super::riff::write_wav_tags(staged, edit),
        Container::Flac => {
            let mut flac = lofty::flac::FlacFile::read_from(&mut open()?, options).map_err(read_error)?;
            let mut vorbis = flac.remove_vorbis_comments().unwrap_or_default();
            apply_vorbis_edit(&mut vorbis, edit);
            flac.set_vorbis_comments(vorbis);
            flac.save_to_path(staged, WriteOptions::default())
        }
        Container::Ogg => {
            let mut ogg = lofty::ogg::VorbisFile::read_from(&mut open()?, options).map_err(read_error)?;
            apply_vorbis_edit(ogg.vorbis_comments_mut(), edit);
            ogg.save_to_path(staged, WriteOptions::default())
        }
        Container::Mp3 => {
            let mut mp3 = lofty::mpeg::MpegFile::read_from(&mut open()?, options).map_err(read_error)?;
            let mut id3 = mp3.remove_id3v2().unwrap_or_default();
            apply_id3_edit(&mut id3, edit);
            mp3.set_id3v2(id3);
            mp3.save_to_path(staged, WriteOptions::default())
        }
        Container::Aiff => {
            let mut aiff = lofty::iff::aiff::AiffFile::read_from(&mut open()?, options).map_err(read_error)?;
            let mut text = aiff.remove_text_chunks().unwrap_or_default();
            apply_aiff_text_edit(&mut text, edit);
            aiff.set_text_chunks(text);
            let mut id3 = aiff.remove_id3v2().unwrap_or_default();
            apply_id3_edit(&mut id3, edit);
            aiff.set_id3v2(id3);
            aiff.save_to_path(staged, WriteOptions::default())
                .map_err(|err| path_io_error("write tags to", staged, err))?;
            return super::riff::move_aiff_tags_before_sound(staged);
        }
    };
    saved.map_err(|err| path_io_error("write tags to", staged, err))
}

/// Write `edit` into the file at `path` (see the module docs for the steps).
pub(crate) fn write_tags(path: &Path, edit: &TagEdit) -> Result<(), String> {
    if edit.is_empty() {
        return Ok(());
    }
    let target = write_target(path);
    let container = Container::detect(&target)
        .ok_or_else(|| format!("Unsupported file type: {}", path.display()))?;
    // Reads pick the parser by extension; a tag written in a format they will
    // not look for would vanish from search.
    if Container::of(&target) != Some(container) {
        return Err(format!(
            "{} does not contain the audio format its extension says",
            path.display()
        ));
    }
    let sidecar_stamp = FileStamp::of(path);

    stage_and_replace(&target, |staged| {
        apply_tag_edit(staged, container, edit)?;
        super::verify::verify_staged_write(&target, staged, container, edit)
    })?;

    // Tundra's own write changes mtime and size. Carry any sidecar row across
    // so labels stored there are not mistaken for a different file's.
    crate::tag_store::restamp(path, sidecar_stamp);
    Ok(())
}

/// Tags go into the file a symlink points at. Replacing the link itself would
/// turn it into a detached copy and leave the real file untagged.
fn write_target(path: &Path) -> PathBuf {
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    if is_link
        && let Ok(target) = crate::path_util::canonical_path(path) {
            return target;
        }
    path.to_path_buf()
}

/// Edits a copy, then swaps it in, so a failed write never truncates the original.
pub(crate) fn stage_and_replace(
    path: &Path,
    edit: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    use crate::safe_write::{ensure_writable, replace_file, sync_file, sync_parent_dir, unique_sidecar};

    let original_perms = std::fs::metadata(path).map_err(|err| path_io_error("read", path, err))?.permissions();
    // Size and mtime, to notice another program writing the file mid-edit.
    let before_stamp = FileStamp::of(path);
    let tmp = unique_sidecar(path, "tag");

    let staged = (|| {
        std::fs::copy(path, &tmp).map_err(|err| format!("Failed to stage {}: {err}", path.display()))?;
        ensure_writable(&tmp).map_err(|err| format!("Failed to prepare tagged file {}: {err}", path.display()))?;
        edit(&tmp)?;
        sync_file(&tmp).map_err(|err| format!("Failed to sync tagged file {}: {err}", tmp.display()))?;
        if FileStamp::of(path) != before_stamp {
            return Err(format!(
                "{} changed on disk while its tags were being written; nothing was saved",
                path.display()
            ));
        }
        ensure_writable(path).map_err(|err| {
            let _ = std::fs::set_permissions(path, original_perms.clone());
            format!("Cannot write tags to read-only file {}: {err}", path.display())
        })
    })();
    let discard = |message: String| {
        let _ = std::fs::remove_file(&tmp);
        Err(message)
    };
    if let Err(message) = staged {
        return discard(message);
    }
    if let Err(err) = replace_file(&tmp, path) {
        if path.exists() {
            let _ = std::fs::set_permissions(path, original_perms);
            return discard(format!("Failed to replace {}: {err}", path.display()));
        }
        // `ReplaceFileW` can fail after moving the original away. The staged
        // copy holds the full audio plus the new tags, so it takes its place.
        if let Err(rename_err) = std::fs::rename(&tmp, path) {
            return Err(format!(
                "Failed to replace {}: {err}. The tagged audio is preserved at {} ({rename_err})",
                path.display(),
                tmp.display()
            ));
        }
        let _ = std::fs::set_permissions(path, original_perms);
        return Err(format!("Failed to replace {}: {err}", path.display()));
    }

    let _ = sync_parent_dir(path);
    let _ = std::fs::set_permissions(path, original_perms);
    Ok(())
}

fn require_audio(path: &Path) -> Result<(), String> {
    if is_audio(path) {
        Ok(())
    } else {
        Err(format!("Not an audio file: {}", path.display()))
    }
}

/// Fields for the sidecar when the container refuses the write. An empty
/// instrument/artist/comment in the editor means "leave alone", as on disk.
fn sidecar_fields_for_fallback(path: &Path, edits: &ManualTagEdits) -> ManualTagEdits {
    let existing = crate::tag_store::manual_fields(path).unwrap_or_default();
    let kept = |edited: &str, existing: String| non_empty(Some(edited)).unwrap_or(existing);
    ManualTagEdits {
        instrument: kept(&edits.instrument, existing.instrument),
        artist: kept(&edits.artist, existing.artist),
        comment: kept(&edits.comment, existing.comment),
        ..edits.trimmed()
    }
}

/// Where a manual edit ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedTo {
    File,
    /// The file could not take the write (the reason is kept); the values live
    /// in Tundra's tag database and only Tundra shows them.
    Sidecar(String),
}

pub fn write_manual_tags(path: &Path, edits: &ManualTagEdits) -> Result<SavedTo, String> {
    require_audio(path)?;

    let edit = TagEdit {
        native: NativeTags {
            instrument: non_empty(Some(&edits.instrument)),
            artist: non_empty(Some(&edits.artist)),
            comment: non_empty(Some(&edits.comment)),
        },
        manual: Some(edits),
    };

    match write_tags(path, &edit) {
        // The file now holds every field, so stale sidecar values must not
        // shadow it on the next read.
        Ok(()) => crate::tag_store::clear_manual_fields(path).map(|()| SavedTo::File),
        Err(disk_err) => crate::tag_store::set_manual_fields(
            path,
            &sidecar_fields_for_fallback(path, edits),
            TUNDRA_TAG_VERSION,
        )
        .map(|()| SavedTo::Sidecar(disk_err.clone()))
        .map_err(|store_err| format!("{disk_err} (sidecar: {store_err})")),
    }
}

/// Native container tags travel with the file. When the container rejects the
/// write the label is recorded in the sidecar store instead, so search still
/// finds the file.
pub fn write_auto_tags(path: &Path, instrument: &str) -> Result<bool, String> {
    require_audio(path)?;
    let NativeInspection {
        native,
        durable_instrument: durable,
        status,
    } = inspect_native(path);
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

    let edit = TagEdit {
        native: pending,
        manual: None,
    };
    match write_tags(path, &edit) {
        Ok(()) => Ok(true),
        Err(native_error) => {
            // Container refused the write (unsupported layout, unwritable file).
            // Record the label in the sidecar store so search still finds it.
            // Never stamp a sidecar over a native instrument: durable reads
            // prefer native, and a sidecar vN would skip retag forever.
            if let Some(instrument) = &edit.native.instrument {
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
