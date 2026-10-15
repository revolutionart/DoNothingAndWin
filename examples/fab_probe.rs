use engine_workspace::{epic::Epic, fab, storage};
fn main() {
    let s = storage::load_session().expect("Connect Epic");
    let items = fab::library(&s, |n| println!("{n} items")).unwrap();
    std::fs::write(
        "work/fab-items.json",
        serde_json::to_vec_pretty(&items).unwrap(),
    )
    .unwrap();
    for item in items
        .iter()
        .filter(|i| i.title.contains("Pivot") || i.title.contains("Paragon: Twinblast"))
        .take(2)
    {
        let v = item.project_versions.last().unwrap();
        let info=Epic::new().unwrap().fab_request(&s,&format!("artifacts/{}/manifest",v.artifact_id),Some(&serde_json::json!({"item_id":item.asset_id,"namespace":item.asset_namespace,"platform":"Windows"}))).unwrap();
        for p in info["downloadInfo"][0]["distributionPoints"]
            .as_array()
            .unwrap()
        {
            let u = reqwest::Url::parse(p["manifestUrl"].as_str().unwrap()).unwrap();
            println!("CDN host: {:?}", u.host_str());
        }
        let (m, _) = fab::package(&s, item, v).unwrap();
        println!(
            "{}: {} files, {} bytes",
            item.title,
            m.files.len(),
            m.files.iter().map(|f| f.size()).sum::<u64>()
        );
        for f in m.files.iter().take(4) {
            println!("{}", f.name);
        }
    }
}
