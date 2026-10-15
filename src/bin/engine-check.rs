use engine_workspace::{epic::Epic, storage};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let result = if args.iter().any(|a| a == "--fixtures") {
        let catalog: serde_json::Value = serde_json::from_str(
            include_str!("../../tests/fixtures/catalog.json").trim_start_matches('\u{feff}'),
        )
        .unwrap();
        let assets: serde_json::Value = serde_json::from_str(
            include_str!("../../tests/fixtures/builds.json").trim_start_matches('\u{feff}'),
        )
        .unwrap();
        engine_workspace::epic::combine(catalog["elements"].as_array().unwrap(), Some(&assets))
    } else {
        let session = if args.iter().any(|a| a == "--catalog") {
            None
        } else {
            storage::load_session()
        };
        Epic::new()
            .and_then(|e| e.check(session.as_ref()))
            .map(|(snapshot, _)| snapshot)
    };
    match result {
        Ok(s) => {
            println!("{}", serde_json::to_string_pretty(&s).unwrap());
            if !s.verified() {
                eprintln!(
                    "PARTIAL: catalog discovery succeeded, but authenticated hotfix coverage is not verified."
                );
                std::process::exit(2);
            }
        }
        Err(e) => {
            eprintln!("Release check failed: {e}");
            std::process::exit(1);
        }
    }
}
