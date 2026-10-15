use engine_workspace::{epic_registration, maintenance, platform, storage};
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--remove-missing") {
        if args.len() != 4 {
            return Err(
                "Usage: engine-register --remove-missing UE_major.minor <absolute missing folder>"
                    .into(),
            );
        }
        epic_registration::unregister_missing(&args[2], std::path::Path::new(&args[3]))?;
        println!(
            "Removed matching Epic registration for {} at {}",
            args[2], args[3]
        );
        return Ok(());
    }
    let path = std::env::args()
        .nth(1)
        .ok_or("Usage: engine-register <engine folder>")?;
    let engine = platform::inspect_engine(std::path::Path::new(&path), "")
        .ok_or("Engine executable/version not found")?;
    let session = storage::load_session();
    let snapshot = session
        .as_ref()
        .map(|s| storage::load_snapshot(&s.account_id))
        .unwrap_or_default();
    let plan = maintenance::prepare(&engine, session.as_ref(), &snapshot)?;
    println!(
        "Registering exact build {} (metadata only)",
        plan.manifest.build
    );
    epic_registration::register(&plan)?;
    println!(
        "Registered {} at {}; no engine payload downloaded",
        plan.manifest.build, path
    );
    Ok(())
}
