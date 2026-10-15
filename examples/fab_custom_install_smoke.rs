use engine_workspace::{buildpatch, fab, storage};
use std::{fs, path::PathBuf};
fn main() -> Result<(), String> {
    let session = storage::load_session().ok_or("Connect Epic first")?;
    let items = fab::library(&session, |_| {})?;
    let item = items
        .iter()
        .find(|i| i.title == "IES Light Profile Pack")
        .ok_or("Test asset not in account")?;
    let branch = item
        .supported_branches()
        .last()
        .cloned()
        .ok_or("No Windows asset versions")?;
    let association = "{11111111-2222-4333-8444-555555555555}";
    assert!(item.import_version(association, "").is_none());
    let (version, package_engine) = item
        .import_version(association, &branch)
        .ok_or("Alternative resolution failed")?;
    let (manifest, _) = fab::package(&session, item, &version)?;
    let bytes: u64 = manifest.files.iter().map(|f| f.size()).sum();
    if bytes > 150 * 1024 * 1024 {
        return Err(format!("Test package too large: {bytes}"));
    }
    let root = PathBuf::from("work").join(format!(
        "fab-custom-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    ));
    fs::create_dir(&root).map_err(|e| e.to_string())?;
    let project = root.join("CustomEngineTest.uproject");
    let descriptor = serde_json::to_vec_pretty(&serde_json::json!({"FileVersion":3,"EngineAssociation":association,"Category":"","Description":"Isolated alternative asset version installation test"})).unwrap();
    fs::write(&project, &descriptor).map_err(|e| e.to_string())?;
    fs::create_dir(root.join("Config")).map_err(|e| e.to_string())?;
    fs::write(
        root.join("Config/DefaultEngine.ini"),
        b"; Preserve original configuration\r\n",
    )
    .map_err(|e| e.to_string())?;
    println!(
        "Downloading {} for UE {} into isolated custom-engine project ({} package bytes)",
        item.title, branch, bytes
    );
    let result = fab::install(&session, item, &version, &root, &package_engine, |s| {
        if !s.starts_with("Downloading") {
            println!("{s}");
        }
    })?;
    let mut checked = 0;
    let mut installed_bytes = 0;
    for file in &manifest.files {
        let path = PathBuf::from(&file.name);
        let parts: Vec<_> = path.iter().collect();
        if let Some(index) = parts
            .iter()
            .position(|p| p.to_string_lossy().eq_ignore_ascii_case("Content"))
        {
            let relative: PathBuf = parts[index..].iter().collect();
            let raw = fs::read(root.join(relative)).map_err(|e| e.to_string())?;
            assert_eq!(buildpatch::digest(&raw), file.sha);
            installed_bytes += raw.len();
            checked += 1;
        }
    }
    assert!(checked > 0);
    assert_eq!(fs::read(&project).unwrap(), descriptor);
    assert_eq!(
        fs::read(root.join("Config/DefaultEngine.ini")).unwrap(),
        b"; Preserve original configuration\r\n"
    );
    let repeat =
        fab::install(&session, item, &version, &root, &package_engine, |_| {}).unwrap_err();
    assert!(repeat.contains("Existing file would be overwritten"));
    assert_eq!(fs::read(&project).unwrap(), descriptor);
    let report = serde_json::json!({"asset": item.title,"alternative_asset_version":branch,"project":fs::canonicalize(&project).unwrap(),"files_hash_verified":checked,"installed_bytes":installed_bytes,"descriptor_byte_identical":true,"engine_association_unchanged":true,"configuration_unchanged":true,"repeat_overwrite_refused":true,"result":result,"scope":"Real authenticated CDN download through the same resolver and installer used by the dialog; isolated custom-engine project, not UI interaction or editor loading"});
    fs::write(
        "outputs/EngineWorkspace/verification-custom-asset-install.json",
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "PASS: {checked} file hashes verified; project descriptor and engine association unchanged; repeated import refuses overwrite."
    );
    Ok(())
}
