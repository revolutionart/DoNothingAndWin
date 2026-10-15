use engine_workspace::{
    installer::{Control, Progress},
    maintenance, platform, storage,
};
use std::sync::{Arc, Mutex};
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .ok_or("Usage: engine-verify <installed engine folder>")?;
    let root = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
    let engine = platform::discover_engines()
        .into_iter()
        .find(|e| std::fs::canonicalize(&e.path).is_ok_and(|p| p == root))
        .ok_or("Engine not discovered")?;
    let session = storage::load_session();
    let snapshot = session
        .as_ref()
        .map(|s| storage::load_snapshot(&s.account_id))
        .unwrap_or_default();
    let plan = maintenance::prepare(&engine, session.as_ref(), &snapshot)?;
    println!(
        "Exact build: {}; {} selected files",
        plan.manifest.build,
        plan.files().len()
    );
    if std::env::args().any(|a| a == "--plan-only") {
        return Ok(());
    }
    let progress: Arc<Mutex<Progress>> = Arc::default();
    let watch = progress.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            let p = watch.lock().unwrap();
            println!(
                "{}: {} files, {:.2}/{:.2} GiB",
                p.phase,
                p.files,
                p.completed as f64 / 1073741824.,
                p.total as f64 / 1073741824.
            );
        }
    });
    println!(
        "{}",
        maintenance::repair(&plan, &Control::default(), progress)?
    );
    Ok(())
}
