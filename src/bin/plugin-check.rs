use engine_workspace::{platform, plugins, storage};
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let path = std::env::args().nth(1).ok_or("Supply engine path")?;
    let root = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let engine = platform::discover_engines()
        .into_iter()
        .find(|e| std::fs::canonicalize(&e.path).ok().as_ref() == Some(&root))
        .ok_or("Unknown engine")?;
    let session = storage::load_session().ok_or("Connect Epic first")?;
    if std::env::args().any(|a| a == "--diagnose") {
        let assets = engine_workspace::epic::Epic::new()?.owned_assets(&session)?;
        println!("Owned UE builds: {}", assets.len());
        let mut seen = std::collections::HashSet::new();
        for a in assets
            .iter()
            .filter(|a| !a["appName"].as_str().unwrap_or("").starts_with("UE_"))
        {
            if seen.insert(a["catalogItemId"].as_str().unwrap_or("")) {
                println!("{}", a["appName"]);
            }
        }
        return Ok(());
    }
    for p in plugins::check(&engine, &session)? {
        println!(
            "{} | installed {} | available {} | {}",
            p.plugin.name,
            p.plugin.version,
            p.available_version.as_deref().unwrap_or("-"),
            p.status
        );
    }
    Ok(())
}
