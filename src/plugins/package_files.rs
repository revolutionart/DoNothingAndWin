//! File selection inside a verified plugin package, relative to its descriptor.
use std::path::Path;

pub(crate) fn compressed_size<'a>(
    manifest: &crate::buildpatch::Manifest,
    files: impl IntoIterator<Item = &'a crate::buildpatch::File>,
) -> Option<u64> {
    let mut chunks = std::collections::HashSet::new();
    files
        .into_iter()
        .flat_map(|file| &file.parts)
        .filter(|part| chunks.insert(&part.guid))
        .try_fold(0u64, |sum, part| {
            let size = manifest.chunks.get(&part.guid)?.size;
            (size > 0).then(|| sum.checked_add(size)).flatten()
        })
}

/// Keep shared plugin content and Windows precompiled data. Platform-specific
/// source code remains intact so publishers' modules can still be rebuilt.
pub(crate) fn windows_file(relative: &Path) -> bool {
    let parts: Vec<_> = relative
        .iter()
        .map(|part| part.to_string_lossy().to_ascii_lowercase())
        .collect();
    match parts.first().map(String::as_str) {
        Some("binaries") if parts.get(1).is_some_and(|p| p == "thirdparty") => {
            !parts.iter().skip(2).any(|p| other_platform(p))
        }
        Some("binaries") => !parts.get(1).is_some_and(|p| other_platform(p)),
        Some("intermediate") if parts.get(1).is_some_and(|p| p == "build") => {
            !parts.get(2).is_some_and(|p| other_platform(p))
        }
        _ => true,
    }
}

fn other_platform(name: &str) -> bool {
    matches!(
        name,
        "mac"
            | "macos"
            | "macosx"
            | "macarm64"
            | "linux"
            | "linuxarm64"
            | "linuxaarch64"
            | "linuxx64"
            | "android"
            | "ios"
            | "tvos"
            | "visionos"
            | "hololens"
            | "ps4"
            | "ps5"
            | "xboxone"
            | "xboxseriesx"
            | "xsx"
            | "switch"
    ) || ["linux-", "linux_", "android-", "android_", "mac-", "mac_"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
pub(crate) fn cached_fixture(cache_root: &Path, engine: &str) -> crate::buildpatch::Manifest {
    use crate::buildpatch::{self, Chunk, File, Manifest, Part};
    let descriptor = serde_json::to_vec(&serde_json::json!({
        "FriendlyName":"Test", "Version":2, "VersionName":"2.0", "EngineVersion":engine
    }))
    .unwrap();
    let mut manifest = Manifest {
        app: "fixture-artifact".into(),
        build: format!("{engine}-test-Windows"),
        ..Default::default()
    };
    let directory = cache_root.join("EngineWorkspace/Chunks");
    std::fs::create_dir_all(&directory).unwrap();
    for (i, (name, bytes)) in [
        ("Test.uplugin", descriptor),
        ("Binaries/Win64/Test.dll", vec![1; 32]),
        (
            "Intermediate/Build/Win64/x64/UnrealGame/Test.cpp.obj",
            vec![2; 64],
        ),
        (
            "Intermediate/Build/Win64/UnrealGame/Test.precompiled",
            vec![3; 64],
        ),
        ("Source/Mac/Module.cpp", vec![4; 32]),
        ("Binaries/Mac/Test.dylib", vec![5; 1024]),
        ("Intermediate/Build/Mac/arm64/Test.o", vec![6; 1024]),
        ("Intermediate/Build/IOS/arm64/Test.o", vec![7; 1024]),
        ("Intermediate/Build/Android/a/Test.o", vec![8; 1024]),
    ]
    .into_iter()
    .enumerate()
    {
        let guid = format!("windows-files-{i}");
        let sha = buildpatch::digest(&bytes);
        std::fs::write(directory.join(buildpatch::hex(&sha)), &bytes).unwrap();
        manifest.files.push(File {
            name: format!("Test/{name}"),
            sha,
            parts: vec![Part {
                guid: guid.clone(),
                offset: 0,
                size: bytes.len() as u32,
            }],
            ..Default::default()
        });
        manifest.chunks.insert(
            guid.clone(),
            Chunk {
                guid,
                sha,
                window: bytes.len() as u32,
                size: bytes.len() as u64,
                ..Default::default()
            },
        );
    }
    manifest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_size_counts_shared_chunks_once_and_excludes_unselected_files() {
        let cache = tempfile::tempdir().unwrap();
        let mut manifest = cached_fixture(cache.path(), "5.7.0");
        let file = manifest.files[1].clone();
        assert_eq!(compressed_size(&manifest, [&file, &file]), Some(32));
        manifest.chunks.get_mut(&file.parts[0].guid).unwrap().size = 0;
        assert_eq!(compressed_size(&manifest, [&file]), None);
    }

    #[test]
    fn only_foreign_platform_build_artifacts_are_skipped() {
        for name in [
            "Binaries/Win64/UnrealEditor-Module.dll",
            "Binaries/Win64/UnrealEditor-Module.pdb",
            "Binaries/ThirdParty/Win64/library.dll",
            "Intermediate/Build/Win64/x64/UnrealGame/Shipping/Module.cpp.obj",
            "Intermediate/Build/Win64/UnrealGame/Module.precompiled",
            "Intermediate/BuildRules/ModuleRules.dll",
            "Source/Mac/Platform.cpp",
            "Source/ThirdParty/Linux/include/library.h",
            "Content/Android/Example.uasset",
            "Resources/Mac/Icon.png",
            "Config/Android/Settings.ini",
            "Shaders/Private/Platform.ush",
            "Module.uplugin",
        ] {
            assert!(
                windows_file(Path::new(name)),
                "Required file removed: {name}"
            );
        }
        for name in [
            "Binaries/Mac/UnrealEditor-Module.dylib",
            "bInArIeS/mAc/Module.dylib",
            "Binaries/Linux/libModule.so",
            "Binaries/ThirdParty/Linux/x86_64/libModule.so",
            "Intermediate/Build/Mac/arm64/UnrealEditor/Module.o",
            "Intermediate/Build/Mac/x64/UnrealGame/Module.o",
            "Intermediate/Build/IOS/arm64/UnrealGame/Module.o",
            "Intermediate/Build/Android/a/UnrealGame/Module.o",
            "Intermediate/Build/LinuxArm64/UnrealGame/Module.o",
        ] {
            assert!(
                !windows_file(Path::new(name)),
                "Foreign build artifact kept: {name}"
            );
        }
    }
}
