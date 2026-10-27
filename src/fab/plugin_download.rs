//! Verified plugin packages saved without engine installation or registration.
use super::{
    Item,
    plugin_install::{descriptor_file, resolve_with},
};
use crate::{
    buildpatch::{self, Manifest},
    epic::Session,
    installer::{self, Control},
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Default)]
pub struct Progress {
    pub phase: String,
    pub file: String,
    pub prepared: u64,
    pub total: u64,
    pub downloaded: u64,
    pub download_bytes: Option<u64>,
    pub files: usize,
    pub total_files: usize,
}

pub fn cache_directory(fab_cache: &str) -> PathBuf {
    Path::new(fab_cache).join("EngineWorkspace/Plugins")
}
/// Identity used by existing Download only folders; keep it shared with verification.
pub(crate) fn cache_fingerprint(manifest: &Manifest) -> Result<String, String> {
    fingerprint(manifest, false)
}
pub(crate) fn windows_cache_fingerprint(manifest: &Manifest) -> Result<String, String> {
    fingerprint(manifest, true)
}
fn fingerprint(manifest: &Manifest, windows_only: bool) -> Result<String, String> {
    let descriptor = installer::relative(&descriptor_file(manifest)?.name)?;
    let prefix = descriptor.parent().ok_or("Invalid plugin root")?;
    let mut files = Vec::new();
    let mut names = HashSet::new();
    for file in &manifest.files {
        let full = installer::relative(&file.name)?;
        if let Ok(relative) = full.strip_prefix(prefix) {
            if relative.as_os_str().is_empty() {
                continue;
            }
            if windows_only && !crate::plugins::package_files::windows_file(relative) {
                continue;
            }
            if !names.insert(relative.to_string_lossy().to_lowercase()) {
                return Err("Duplicate plugin package path".into());
            }
            files.push((file, relative.to_owned()));
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    let mut identity = format!("{}\n{}\n", manifest.app, manifest.build).into_bytes();
    for (file, relative) in files {
        identity.extend_from_slice(relative.to_string_lossy().as_bytes());
        identity.push(0);
        identity.extend_from_slice(&file.sha);
        identity.extend_from_slice(&file.size().to_le_bytes());
    }
    Ok(buildpatch::hex(&buildpatch::digest(&identity)))
}

pub fn download(
    session: &Session,
    selected: &Item,
    branch: &str,
    location: &Path,
    wait_for_library: bool,
    control: &Control,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    control.check()?;
    crate::download_policy::check_install_allowed()?;
    progress(Progress {
        phase: "Checking available plugin builds…".into(),
        ..Default::default()
    });
    let owned = super::library(session, |_| {})?;
    control.check()?;
    let item = owned
        .iter()
        .find(|i| i.plugin() && i.key() == selected.key())
        .ok_or("This plugin is no longer available in the connected Fab library")?;
    let candidate = resolve_with(item, branch, wait_for_library, |version| {
        control.check()?;
        let (manifest, bases) = super::package(session, item, version)?;
        control.check()?;
        let file = descriptor_file(&manifest)?;
        let raw = crate::plugins::payload(&manifest, &bases, file, &mut HashMap::new())?;
        let descriptor = serde_json::from_slice(&raw).map_err(|_| "Invalid plugin descriptor")?;
        Ok((manifest, bases, descriptor))
    })?;
    extract(
        &candidate.manifest,
        &candidate.bases,
        branch,
        location,
        control,
        &mut HashMap::new(),
        progress,
    )
    .map_err(|error| {
        if error.starts_with("Stopped.") {
            "Stopped. Partial plugin output was removed; downloaded chunks remain cached.".into()
        } else {
            error
        }
    })
}

fn extract(
    manifest: &Manifest,
    bases: &[reqwest::Url],
    branch: &str,
    location: &Path,
    control: &Control,
    cache: &mut HashMap<String, Vec<u8>>,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    control.check()?;
    let branch = crate::model::EngineVersion::parse(branch)
        .ok_or("Choose a supported UE version")?
        .branch();
    let descriptor = installer::relative(&descriptor_file(manifest)?.name)?;
    let prefix = descriptor.parent().ok_or("Invalid plugin root")?;
    let name = descriptor.file_stem().ok_or("Invalid plugin name")?;
    let mut files = Vec::new();
    let mut names = HashSet::new();
    for file in &manifest.files {
        let full = installer::relative(&file.name)?;
        if let Ok(relative) = full.strip_prefix(prefix) {
            if relative.as_os_str().is_empty() {
                continue;
            }
            if !crate::plugins::package_files::windows_file(relative) {
                continue;
            }
            if !names.insert(relative.to_string_lossy().to_lowercase()) {
                return Err("Duplicate plugin package path".into());
            }
            files.push((file, relative.to_owned()));
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    let fingerprint = windows_cache_fingerprint(manifest)?;
    if !location.is_absolute() {
        return Err("Choose an absolute download folder.".into());
    }
    installer::no_links(location)?;
    fs::create_dir_all(location).map_err(|e| format!("Cannot create download folder: {e}"))?;
    let root = fs::canonicalize(location).map_err(|e| e.to_string())?;
    let parent = root.join(name);
    installer::no_links(&parent)?;
    fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let destination = parent.join(format!("UE_{branch}-{}", &fingerprint[..16]));
    let plugin = destination.join(name);
    installer::no_links(&plugin)?;
    let mut state = Progress {
        phase: "Downloading plugin files…".into(),
        total: files
            .iter()
            .try_fold(0u64, |n, (f, _)| n.checked_add(f.size()))
            .ok_or("Plugin is too large")?,
        total_files: files.len(),
        download_bytes: crate::plugins::package_files::compressed_size(
            manifest,
            files.iter().map(|(f, _)| *f),
        ),
        ..Default::default()
    };
    if destination.exists() {
        state.phase = "Checking saved plugin files…".into();
        progress(state.clone());
        for (file, relative) in &files {
            control.check()?;
            let path = plugin.join(relative);
            installer::no_links(&path)?;
            let digest = installer::hash_file_progress(&path, &mut |_| control.check());
            control.check()?;
            if digest.as_ref().ok() != Some(&file.sha) {
                return Err(format!(
                    "Saved plugin differs from this package. Choose another folder or remove the old download: {}",
                    destination.display()
                ));
            }
            state.prepared += file.size();
            state.files += 1;
            state.file = relative.to_string_lossy().into_owned();
            progress(state.clone());
        }
        state.phase = "Plugin already downloaded and verified".into();
        progress(state);
        crate::vault_health::write_windows_download_index(
            &destination,
            manifest,
            prefix,
            Path::new(name),
        )?;
        return Ok(plugin);
    }
    if fs2::available_space(&root).map_err(|e| e.to_string())?
        < state.total.saturating_add(64 * 1024 * 1024)
    {
        return Err("Not enough space to download this plugin.".into());
    }
    let stage = parent.join(format!(
        ".engine-workspace-plugin-download-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let staged_plugin = stage.join(name);
        fs::create_dir(&staged_plugin).map_err(|e| e.to_string())?;
        for (file, relative) in &files {
            control.check()?;
            state.file = relative.to_string_lossy().into_owned();
            let path = staged_plugin.join(relative);
            installer::no_links(&path)?;
            fs::create_dir_all(path.parent().ok_or("Invalid plugin path")?)
                .map_err(|e| e.to_string())?;
            let mut received = 0;
            let mut last = std::time::Instant::now();
            crate::plugins::payload_to_file_controlled(
                manifest,
                bases,
                file,
                cache,
                &path,
                control,
                &mut |written, network| {
                    received = network;
                    if last.elapsed().as_millis() >= 100 || written == file.size() {
                        let mut p = state.clone();
                        p.prepared += written;
                        p.downloaded += network;
                        progress(p);
                        last = std::time::Instant::now();
                    }
                },
            )?;
            state.prepared += file.size();
            state.downloaded += received;
            state.files += 1;
            progress(state.clone());
        }
        state.phase = "Saving verified plugin…".into();
        progress(state.clone());
        control.check()?;
        crate::vault_health::write_windows_download_index(
            &stage,
            manifest,
            prefix,
            Path::new(name),
        )?;
        installer::no_links(&destination)?;
        if destination.exists() {
            return Err(
                "Download destination appeared; existing files were not overwritten.".into(),
            );
        }
        let _change = crate::vault_health::changing(&destination);
        fs::rename(&stage, &destination)
            .map_err(|e| format!("Cannot save downloaded plugin: {e}"))?;
        state.phase = "Plugin downloaded".into();
        state.file.clear();
        progress(state);
        Ok(plugin)
    })();
    if stage.exists() {
        installer::no_links(&stage)?;
        fs::remove_dir_all(&stage)
            .map_err(|e| format!("Could not remove partial download {}: {e}", stage.display()))?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Downloads an owned Fab plugin to a temporary folder; requires a connected account and PLUGIN_DOWNLOAD_TEST_TITLE"]
    fn live_owned_plugin_download_without_engine() {
        let title =
            std::env::var("PLUGIN_DOWNLOAD_TEST_TITLE").expect("Set the exact owned plugin title");
        let session = crate::storage::load_session().expect("Connect Epic first");
        let owned = super::super::load_library_cache(&session.account_id);
        let item = owned
            .iter()
            .find(|item| item.plugin() && item.title == title)
            .expect("Owned plugin title");
        let branch = item
            .supported_branches()
            .last()
            .cloned()
            .expect("Windows branch");
        let dir = tempfile::tempdir().unwrap();
        let mut phase = String::new();
        let path = download(
            &session,
            item,
            &branch,
            dir.path(),
            true,
            &Control::default(),
            |p| {
                if phase != p.phase {
                    println!("{}: {} / {} files", p.phase, p.files, p.total_files);
                    phase = p.phase;
                }
            },
        )
        .expect("Download verified plugin");
        assert!(fs::read_dir(&path).unwrap().any(|f| {
            f.unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "uplugin")
        }));
        assert!(!path.join(".dnw-plugin-install.json").exists());
        assert!(
            path.parent()
                .unwrap()
                .join(crate::vault_health::INDEX)
                .is_file()
        );
        let records = crate::vault_health::verify_roots(
            &[dir.path().to_string_lossy().into()],
            &crate::vault_health::Selection::All,
            &crate::vault_health::Gate::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(records[0].health, crate::vault_health::Health::Healthy);
        assert!(!dir.path().join("Engine").exists());
        println!(
            "Verified {title} for UE {branch}; no engine installation or registration required"
        );
    }
    fn fixture() -> (Manifest, HashMap<String, Vec<u8>>) {
        let mut manifest = Manifest {
            app: "owned-artifact".into(),
            build: "5.8.0-test".into(),
            ..Default::default()
        };
        let mut cache = HashMap::new();
        for (i, (name, data)) in [
            (
                "Engine/Plugins/Marketplace/Test/Test.uplugin",
                br#"{"EngineVersion":"5.8.0","Version":1}"#.to_vec(),
            ),
            (
                "Engine/Plugins/Marketplace/Test/Source/Module.cpp",
                vec![7; 1024 * 1024],
            ),
            (
                "Engine/Plugins/Marketplace/Test/Binaries/Win64/Test.dll",
                vec![8; 42],
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let guid = format!("download-test-{i}");
            manifest.files.push(buildpatch::File {
                name: name.into(),
                sha: buildpatch::digest(&data),
                parts: vec![buildpatch::Part {
                    guid: guid.clone(),
                    offset: 0,
                    size: data.len() as u32,
                }],
                ..Default::default()
            });
            manifest.chunks.insert(guid.clone(), Default::default());
            cache.insert(guid, data);
        }
        (manifest, cache)
    }
    #[test]
    fn windows_plugin_package_excludes_other_platforms_but_keeps_source_and_windows_build_data() {
        let dir = tempfile::tempdir().unwrap();
        let (mut manifest, mut cache) = fixture();
        let included = [
            "Intermediate/Build/Win64/x64/UnrealGame/Test.cpp.obj",
            "Intermediate/Build/Win64/UnrealGame/Test.precompiled",
            "Source/Mac/Platform.cpp",
            "Content/Mac/Example.uasset",
        ];
        let excluded = [
            "Binaries/Mac/Test.dylib",
            "Intermediate/Build/Mac/arm64/UnrealGame/Test.cpp.o",
            "Intermediate/Build/IOS/arm64/UnrealGame/Test.cpp.o",
            "Intermediate/Build/Android/a/UnrealGame/Test.cpp.o",
            "Binaries/Linux/libTest.so",
        ];
        let original_bytes: u64 = manifest.files.iter().map(|f| f.size()).sum();
        for (i, name) in included.iter().chain(&excluded).enumerate() {
            let bytes = vec![i as u8; 1024];
            let guid = format!("platform-{i}");
            manifest.files.push(buildpatch::File {
                name: format!("Engine/Plugins/Marketplace/Test/{name}"),
                sha: buildpatch::digest(&bytes),
                parts: vec![buildpatch::Part {
                    guid: guid.clone(),
                    offset: 0,
                    size: bytes.len() as u32,
                }],
                ..Default::default()
            });
            manifest.chunks.insert(guid.clone(), Default::default());
            cache.insert(guid, bytes);
        }
        let mut progress = Progress::default();
        let plugin = extract(
            &manifest,
            &[],
            "5.8",
            dir.path(),
            &Control::default(),
            &mut cache,
            |p| progress = p,
        )
        .unwrap();
        for file in excluded {
            assert!(
                !plugin.join(file).exists(),
                "Downloaded non-Windows artifact: {file}"
            );
        }
        for file in included {
            assert!(plugin.join(file).is_file(), "Dropped required file: {file}");
        }
        assert_eq!(
            progress.total,
            original_bytes + included.len() as u64 * 1024
        );
        assert_eq!(progress.total_files, 3 + included.len());
        let health = crate::vault_health::verify_roots(
            &[dir.path().to_string_lossy().into()],
            &crate::vault_health::Selection::All,
            &crate::vault_health::Gate::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            health[0].health,
            crate::vault_health::Health::Healthy,
            "Skipped platforms must not become missing-file errors"
        );
    }
    #[test]
    fn extracts_without_an_engine_preserves_source_and_binaries_and_reuses_verified_files() {
        let dir = tempfile::tempdir().unwrap();
        let (m, mut cache) = fixture();
        let mut events = Vec::new();
        let path = extract(
            &m,
            &[],
            "5.8",
            dir.path(),
            &Control::default(),
            &mut cache,
            |p| events.push(p),
        )
        .unwrap();
        assert!(path.join("Test.uplugin").is_file());
        assert_eq!(
            fs::read(path.join("Source/Module.cpp")).unwrap(),
            vec![7; 1024 * 1024]
        );
        assert_eq!(
            fs::read(path.join("Binaries/Win64/Test.dll")).unwrap(),
            vec![8; 42]
        );
        assert!(!dir.path().join("Engine").exists());
        assert!(!path.join(".dnw-plugin-install.json").exists());
        assert_eq!(
            events.last().unwrap().prepared,
            events.last().unwrap().total
        );
        assert_eq!(
            extract(
                &m,
                &[],
                "5.8",
                dir.path(),
                &Control::default(),
                &mut HashMap::new(),
                |_| {}
            )
            .unwrap(),
            path
        );
        fs::write(path.join("Test.uplugin"), "custom").unwrap();
        assert!(
            extract(
                &m,
                &[],
                "5.8",
                dir.path(),
                &Control::default(),
                &mut cache,
                |_| {}
            )
            .is_err()
        );
        assert_eq!(fs::read(path.join("Test.uplugin")).unwrap(), b"custom");
    }
    #[test]
    fn corruption_cancellation_and_unsafe_paths_never_publish_a_plugin() {
        for kind in 0..4 {
            let dir = tempfile::tempdir().unwrap();
            let (mut m, mut cache) = fixture();
            if kind == 0 {
                m.files[0].sha = [0; 20];
            }
            if kind == 2 {
                m.files[1].name = "../escape".into();
            }
            if kind == 3 {
                m.files.push(m.files[1].clone());
            }
            let control = Control::default();
            assert!(
                extract(&m, &[], "5.8", dir.path(), &control, &mut cache, |p| {
                    if kind == 1 && p.prepared > 0 {
                        control.stop();
                    }
                })
                .is_err()
            );
            let parent = dir.path().join("Test");
            assert!(!parent.exists() || fs::read_dir(parent).unwrap().next().is_none());
        }
    }
    #[test]
    fn different_builds_are_saved_separately() {
        let dir = tempfile::tempdir().unwrap();
        let (mut m, mut cache) = fixture();
        let a = extract(
            &m,
            &[],
            "5.8",
            dir.path(),
            &Control::default(),
            &mut cache,
            |_| {},
        )
        .unwrap();
        m.build = "5.8.0-new-build".into();
        let b = extract(
            &m,
            &[],
            "5.8",
            dir.path(),
            &Control::default(),
            &mut cache,
            |_| {},
        )
        .unwrap();
        assert_ne!(a, b);
        assert!(a.join("Test.uplugin").is_file() && b.join("Test.uplugin").is_file());
    }
}
