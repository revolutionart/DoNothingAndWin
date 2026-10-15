use engine_workspace::{
    buildpatch, discovery,
    installer::{self, Control, Progress},
    model::Release,
    storage,
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str){
 Some("inspect")=>{let bytes=std::fs::read(args.get(2).ok_or("Supply manifest file")?).map_err(|e|e.to_string())?;let m=buildpatch::parse(&bytes)?;let mut tags:Vec<_>=m.files.iter().flat_map(|f|f.tags.clone()).collect();tags.sort();tags.dedup();println!("{}",serde_json::json!({"app":m.app,"build":m.build,"feature":m.feature,"files":m.files.len(),"chunks":m.chunks.len(),"install_bytes":m.files.iter().map(|f|f.size()).sum::<u64>(),"tags":tags}));},
 Some("discover")=>{let roots=if let Some(p)=args.get(2){vec![p.into()]}else{discovery::roots()};let r=discovery::scan(&roots,Arc::default(),Arc::default());let mut p=storage::load_preferences();let added=discovery::merge(&mut p.projects,r.projects);storage::save_preferences(&p)?;println!("{}",serde_json::json!({"projects":p.projects.len(),"added":added,"directories":r.directories,"skipped":r.skipped}));},
 Some("prepare")|Some("install")=>{
  let session=storage::load_session().ok_or("Sign in through DoNothing&Win Launcher first")?;
  let app=args.get(2).ok_or("Supply UE_x.y")?;let parent=Path::new(args.get(3).ok_or("Supply destination parent")?);
  let snapshot=storage::load_snapshot(&session.account_id);let release:Release=snapshot.releases.iter().find(|r|&r.app==app).cloned().ok_or("Run the signed-in release check in the app first")?;
  let mut plan=installer::prepare(&session,&release,parent)?;
  if args.iter().any(|a|a=="--core"){plan.selected=plan.tags.iter().filter(|s|matches!(s.as_str(),"Core"|"Windows"|"Win64")).cloned().collect();}
  let (disk,download,cache)=plan.sizes();let summary=serde_json::json!({"app":plan.manifest.app,"build":plan.manifest.build,"target":plan.target,"files":plan.files().len(),"download_bytes":download,"install_bytes":disk,"cache_bytes":cache,"tags":plan.tags,"selected_tags":plan.selected});println!("{summary}");storage::write_json(&storage::data_dir().join("last-install-plan.json"),&summary)?;
  if args[1]=="install"{let progress:Arc<Mutex<Progress>>=Arc::default();let watch=progress.clone();let stop=Arc::new(std::sync::atomic::AtomicBool::new(false));let done=stop.clone();let started=Instant::now();let log=std::thread::spawn(move||{while !done.load(std::sync::atomic::Ordering::Relaxed){std::thread::sleep(std::time::Duration::from_secs(10));let p=watch.lock().unwrap();println!("{} {:.2}/{:.2} GiB; {} verified files; elapsed {}s",p.phase,p.completed as f64/1073741824.,p.total as f64/1073741824.,p.files,started.elapsed().as_secs());}});let result=installer::install(&plan,&Control::default(),progress);stop.store(true,std::sync::atomic::Ordering::Relaxed);let _=log.join();let root=result?;let mut prefs=storage::load_preferences();let path=root.to_string_lossy().to_string();if !prefs.engine_paths.contains(&path){prefs.engine_paths.push(path);}storage::save_preferences(&prefs)?;println!("Installed and verified: {}",root.display());}
 },
 _=>return Err("Commands: inspect <manifest>, discover [root], prepare|install <UE_x.y> <parent> [--core]".into())}
    Ok(())
}
