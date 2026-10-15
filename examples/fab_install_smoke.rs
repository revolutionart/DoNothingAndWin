use engine_workspace::{buildpatch, fab, storage};
fn main() {
    let s = storage::load_session().expect("Sign in");
    let items: Vec<fab::Item> =
        serde_json::from_slice(&std::fs::read("work/fab-items.json").unwrap()).unwrap();
    for title in ["IES Light Profile Pack"] {
        let item = items.iter().find(|i| i.title == title).unwrap();
        let version = item.project_versions.last().unwrap();
        let engine = version.engine_versions.last().unwrap();
        let (m, _) = fab::package(&s, item, version).unwrap();
        let bytes: u64 = m.files.iter().map(|f| f.size()).sum();
        assert!(bytes < 150 * 1024 * 1024, "Fixture too large: {bytes}");
        let root = std::path::PathBuf::from("work/fab-smoke").join(if item.plugin() {
            "Engine"
        } else {
            "Project"
        });
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("keep.txt"), b"preserve me").unwrap();
        println!("Testing {title}: {bytes} bytes, {engine}");
        println!(
            "{}",
            fab::install(&s, item, version, &root, engine, |_| {}).unwrap()
        );
        let prefix = if item.plugin() {
            std::path::Path::new(
                &m.files
                    .iter()
                    .find(|f| f.name.ends_with(".uplugin"))
                    .unwrap()
                    .name,
            )
            .parent()
            .unwrap()
            .to_owned()
        } else {
            std::path::PathBuf::new()
        };
        let mut checked = 0;
        for f in &m.files {
            let path = if item.plugin() {
                root.join("Engine/Plugins/Marketplace")
                    .join(prefix.file_name().unwrap())
                    .join(std::path::Path::new(&f.name).strip_prefix(&prefix).unwrap())
            } else {
                root.join(&f.name)
            };
            if path.is_file() {
                assert_eq!(buildpatch::digest(&std::fs::read(path).unwrap()), f.sha);
                checked += 1;
            }
        }
        assert_eq!(
            std::fs::read(root.join("keep.txt")).unwrap(),
            b"preserve me"
        );
        assert!(checked > 0);
        println!("Verified {checked} hashes and preserved existing file");
        assert!(fab::install(&s, item, version, &root, engine, |_| {}).is_err());
        println!("Repeated installation correctly refused overwrite");
    }
}
