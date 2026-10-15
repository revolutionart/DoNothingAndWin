use engine_workspace::{
    installer::Control,
    maintenance,
    model::{Installation, Snapshot},
};
#[test]
#[ignore = "Requires an existing exact-build repair manifest and Epic CDN access"]
fn live_cdn_restores_missing_and_corrupted_epic_files() {
    let source = std::env::var("ENGINE_REPAIR_TEST_SOURCE").expect("ENGINE_REPAIR_TEST_SOURCE");
    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::path::Path::new(&source).join(".engine-workspace-install.json"))
            .unwrap(),
    )
    .unwrap();
    let engine = Installation {
        path: source.clone(),
        version: receipt["build"].as_str().unwrap().into(),
        association: "5.8".into(),
    };
    let mut plan = maintenance::prepare(&engine, None, &Snapshot::default()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    plan.target = temp.path().join("UE_test");
    std::fs::create_dir_all(plan.target.join("Engine/Binaries/Win64")).unwrap();
    std::fs::write(
        plan.target.join(".engine-workspace-install.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    plan.manifest.files.retain(|f| {
        matches!(
            f.name.as_str(),
            "Engine/Binaries/Win64/UnrealEditor.exe" | "Engine/Build/Build.version"
        )
    });
    assert_eq!(plan.files().len(), 2);
    let exe = plan
        .manifest
        .files
        .iter()
        .find(|f| f.name.ends_with(".exe"))
        .unwrap();
    std::fs::write(plan.target.join(&exe.name), vec![0; exe.size() as usize]).unwrap();
    let progress = std::sync::Arc::new(std::sync::Mutex::new(Default::default()));
    let result = maintenance::repair(&plan, &Control::default(), progress.clone()).unwrap();
    assert!(result.contains("repaired 2"));
    assert!(progress.lock().unwrap().network > 0);
    for f in plan.files() {
        let got = std::fs::read(plan.target.join(&f.name)).unwrap();
        assert_eq!(engine_workspace::buildpatch::digest(&got), f.sha);
    }
    println!(
        "{result} Downloaded {} bytes from Epic CDN into isolated fixture.",
        progress.lock().unwrap().network
    );
}
