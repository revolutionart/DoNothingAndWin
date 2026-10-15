use engine_workspace::{epic::Epic, fab, installer, storage};
fn main() {
    let s = storage::load_session().unwrap();
    let items: Vec<fab::Item> =
        serde_json::from_slice(&std::fs::read("work/fab-items.json").unwrap()).unwrap();
    let i = items
        .iter()
        .find(|i| i.title == "IES Light Profile Pack")
        .unwrap();
    let v = i.project_versions.last().unwrap();
    let info=Epic::new().unwrap().fab_request(&s,&format!("artifacts/{}/manifest",v.artifact_id),Some(&serde_json::json!({"item_id":i.asset_id,"namespace":i.asset_namespace,"platform":"Windows"}))).unwrap();
    let u = reqwest::Url::parse(
        info["downloadInfo"][0]["distributionPoints"][0]["manifestUrl"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let raw = installer::fetch(&installer::client().unwrap(), u, 128 * 1024 * 1024).unwrap();
    std::fs::write("work/fab-legacy-manifest.json", raw).unwrap();
}
