use super::{Control, Manifest, UpdateProgress, buildpatch, installer, payload_to_file_io};
use crate::package_chunks::{self, ChunkSource};
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const REPORT_INTERVAL: Duration = Duration::from_millis(150);

pub(super) fn prepare(
    manifest: &Manifest,
    bases: &[reqwest::Url],
    files: &[(&buildpatch::File, PathBuf)],
    installed: &Path,
    stage: &Path,
    transfer: &Control,
    state: &mut UpdateProgress,
    report: &mut impl FnMut(UpdateProgress),
) -> Result<(), String> {
    let mut last = Instant::now();
    report(state.clone());
    let mut publish = |state: &UpdateProgress| {
        if last.elapsed() >= REPORT_INTERVAL {
            report(state.clone());
            last = Instant::now();
        }
    };
    let mut pending = Vec::with_capacity(files.len());
    for (file, relative) in files {
        transfer.check()?;
        let original = installed.join(relative);
        let dest = stage.join(relative);
        installer::no_links(&dest)?;
        fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        state.file = relative.to_string_lossy().into_owned();
        if copy_verified(&original, &dest, file, |read, written| {
            state.read += read;
            state.written += written;
            publish(state);
            transfer.check()
        })? {
            state.prepared += file.size();
            state.files_done += 1;
            publish(state);
        } else {
            pending.push((*file, relative));
        }
    }

    let chunks = package_chunks::ordered_chunks(manifest, pending.iter().map(|(file, _)| *file))?;
    let control = transfer.child();
    let source = ChunkSource::new(manifest, bases, &control)?;
    let mut cache = HashMap::new();
    let local_reads = state.read;
    let mut file_writes = state.written;
    package_chunks::with_prefetch(
        chunks,
        package_chunks::Connections::ADAPTIVE,
        &control,
        |chunk| source.load(chunk),
        || source.io(),
        |prefetch| {
            for (file, relative) in pending {
                state.file = relative.to_string_lossy().into_owned();
                let prepared = state.prepared;
                let result = payload_to_file_io(
                    manifest,
                    bases,
                    file,
                    &mut cache,
                    &stage.join(relative),
                    &control,
                    &mut |io| {
                        let chunks = source.io();
                        state.prepared = prepared + io.prepared;
                        state.downloaded = chunks.downloaded;
                        state.read = local_reads + chunks.read;
                        state.written = file_writes + io.written + chunks.written;
                        publish(state);
                    },
                    |chunk, heartbeat| prefetch.take_with_progress(chunk, heartbeat).map(Some),
                );
                if result.is_err() {
                    control.stop();
                    return result;
                }
                file_writes += file.size();
                state.files_done += 1;
                publish(state);
            }
            Ok(())
        },
    )?;
    let chunks = source.io();
    transfer.check()?;
    state.downloaded = chunks.downloaded;
    state.read = local_reads + chunks.read;
    state.written = file_writes + chunks.written;
    report(state.clone());
    Ok(())
}

// Verify the bytes copied to staging against the new manifest. A separate copy
// keeps the previous version's backup independent of the updated installation.
fn copy_verified(
    original: &Path,
    dest: &Path,
    file: &buildpatch::File,
    mut progress: impl FnMut(u64, u64) -> Result<(), String>,
) -> Result<bool, String> {
    if !fs::metadata(original).is_ok_and(|meta| meta.is_file() && meta.len() == file.size())
        || installer::no_links(original).is_err()
    {
        return Ok(false);
    }
    let Ok(input) = fs::File::open(original) else {
        return Ok(false);
    };
    let mut input = input.take(file.size());
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| format!("Creating {}: {e}", dest.display()))?;
    let mut hash = Sha1::new();
    let mut buffer = [0; 256 * 1024];
    let mut copied = 0;
    while let Ok(count) = input.read(&mut buffer) {
        if count == 0 {
            break;
        }
        progress(count as u64, 0)?;
        let bytes = &buffer[..count];
        output
            .write_all(bytes)
            .map_err(|e| format!("Writing {}: {e}", dest.display()))?;
        hash.update(bytes);
        copied += count as u64;
        progress(0, count as u64)?;
    }
    let actual: [u8; 20] = hash.finalize().into();
    if copied == file.size() && actual == file.sha {
        output.sync_all().map_err(|e| e.to_string())?;
        return Ok(true);
    }
    drop(output);
    fs::remove_file(dest).map_err(|e| e.to_string())?;
    Ok(false)
}
