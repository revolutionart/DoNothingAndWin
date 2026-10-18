//! Authoritative accepted plugin comparisons; every view derives from these rows.
use super::{Checked, Plugin};
use crate::{fab::Item, fab_listing};
use std::{collections::HashMap, time::Instant};

#[derive(Default)]
pub struct Catalogue {
    engines: HashMap<String, (Instant, Vec<Checked>)>,
}
impl Catalogue {
    pub fn clear(&mut self) {
        self.engines.clear();
    }
    pub fn rows(&self, engine: &str) -> &[Checked] {
        self.engines
            .get(engine)
            .map(|(_, rows)| rows.as_slice())
            .unwrap_or_default()
    }
    pub fn checked(&mut self, engine: String, started: Instant, rows: Vec<Checked>) {
        if self
            .engines
            .get(&engine)
            .is_some_and(|(newer, _)| *newer > started)
        {
            return;
        }
        self.engines.insert(engine, (started, rows));
    }
    pub fn checked_row(&mut self, engine: String, started: Instant, row: Checked) {
        let mut rows = self.rows(&engine).to_vec();
        rows.retain(|previous| previous.plugin.descriptor != row.plugin.descriptor);
        rows.push(row);
        self.checked(engine, started, rows);
    }
    pub fn installed(&mut self, engine: String, completed: Instant, plugin: Plugin) {
        self.checked_row(
            engine,
            completed,
            Checked {
                available_version: Some(plugin.version.clone()),
                plugin,
                update: None,
                status: "Up to date".into(),
            },
        );
    }
    pub fn retain_installed(&mut self, engine: &str, plugins: &[Plugin]) {
        if let Some((_, rows)) = self.engines.get_mut(engine) {
            rows.retain(|row| {
                plugins.iter().any(|p| {
                    p.descriptor == row.plugin.descriptor
                        && p.version == row.plugin.version
                        && p.number == row.plugin.number
                })
            });
        }
    }
    pub fn for_item(&self, item: &Item) -> Vec<(String, String)> {
        self.rows_for_item(item)
            .into_iter()
            .map(|(engine, row)| {
                (
                    engine.to_owned(),
                    format!("{}: {}", row.plugin.name, row.status_label()),
                )
            })
            .collect()
    }
    /// Matching pending comparisons, including notices that require Epic's launcher.
    pub fn rows_for_item(&self, item: &Item) -> Vec<(&str, &Checked)> {
        if !item.plugin() {
            return vec![];
        }
        let mut result = Vec::new();
        for (engine, (_, rows)) in &self.engines {
            for row in rows.iter().filter(|r| r.has_update()) {
                let listing = super::fab_listing_id(&row.plugin);
                if row.plugin.catalog_id.as_deref() == Some(item.asset_id.as_str())
                    || row.update.as_ref().is_some_and(|u| {
                        item.project_versions
                            .iter()
                            .any(|v| v.artifact_id == u.manifest.app)
                    })
                    || listing
                        .as_ref()
                        .is_some_and(|id| fab_listing::listing_id(item).as_ref() == Some(id))
                {
                    result.push((engine.as_str(), row));
                }
            }
        }
        result.sort_by(|(a, row_a), (b, row_b)| {
            a.cmp(b)
                .then_with(|| row_a.plugin.name.cmp(&row_b.plugin.name))
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn item(id: &str) -> Item {
        serde_json::from_value(serde_json::json!({"assetId":id,"assetNamespace":"publisher","title":id,"distributionMethod":"CODE_PLUGIN"})).unwrap()
    }
    fn update(id: &str) -> Checked {
        Checked {
            plugin: Plugin {
                name: id.into(),
                version: "1".into(),
                number: 1,
                descriptor: format!("{id}.uplugin").into(),
                catalog_id: Some(id.into()),
            },
            available_version: Some("2".into()),
            update: None,
            status: "Epic integration update available; update through Epic".into(),
        }
    }
    #[test]
    fn direct_update_keeps_the_clicked_plugin_and_engine_paired_with_its_payload() {
        let mut c = Catalogue::default();
        let now = Instant::now();
        let mut first = update("one");
        first.update = Some(super::super::Update {
            version: "2".into(),
            number: 2,
            manifest: crate::buildpatch::Manifest::default(),
            bases: vec![],
            prefix: "one".into(),
        });
        let mut second = first.clone();
        second.update.as_mut().unwrap().version = "3".into();
        c.checked("b".into(), now, vec![second]);
        c.checked("a".into(), now, vec![update("unrelated"), first.clone()]);
        let rows = c.rows_for_item(&item("one"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "a");
        assert_eq!(rows[0].1.plugin.name, "one");
        assert_eq!(rows[0].1.update.as_ref().unwrap().version, "2");
        assert_eq!(rows[1].0, "b");
        assert_eq!(rows[1].1.update.as_ref().unwrap().version, "3");
        c.installed("a".into(), now, first.plugin);
        let remaining = c.rows_for_item(&item("one"));
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].0, "b");
        // An Epic-only notice must never become a runnable update for another plugin.
        assert!(c.rows_for_item(&item("unrelated"))[0].1.update.is_none());
    }
    #[test]
    fn completion_clears_all_projections_without_hiding_other_engines_or_pending_rows() {
        let mut c = Catalogue::default();
        let now = Instant::now();
        c.checked("a".into(), now, vec![update("one"), update("two")]);
        c.checked("b".into(), now, vec![update("one")]);
        let mut installed = update("one").plugin;
        installed.version = "2".into();
        installed.number = 2;
        c.installed("a".into(), now + Duration::from_secs(1), installed.clone());
        c.checked("a".into(), now, vec![update("one")]);
        assert_eq!(c.for_item(&item("one"))[0].0, "b");
        assert_eq!(c.for_item(&item("one")).len(), 1);
        assert_eq!(c.for_item(&item("two")).len(), 1);
        let row = c.rows("a").iter().find(|r| r.plugin.name == "one").unwrap();
        assert_eq!(row.status_label(), "Up to date");
        assert_eq!(row.plugin.version, "2");
        c.installed("b".into(), now + Duration::from_secs(2), installed);
        assert!(c.for_item(&item("one")).is_empty());
    }
    #[test]
    fn newer_checks_clear_updates_and_stale_incremental_results_cannot_restore_them() {
        let mut c = Catalogue::default();
        let now = Instant::now();
        c.checked("a".into(), now, vec![update("one"), update("two")]);
        let mut current = update("one");
        current.status = "Up to date".into();
        c.checked_row("a".into(), now + Duration::from_secs(1), current);
        c.checked_row("a".into(), now, update("one"));
        assert!(c.for_item(&item("one")).is_empty());
        assert_eq!(c.rows("a").len(), 2);
        assert_eq!(c.for_item(&item("two")).len(), 1);
        c.checked("a".into(), now + Duration::from_secs(2), vec![]);
        c.checked("a".into(), now, vec![update("one")]);
        assert!(c.rows("a").is_empty());
    }
    #[test]
    fn unmatched_rows_remain_available_and_new_library_items_need_no_recheck() {
        let mut c = Catalogue::default();
        c.checked("a".into(), Instant::now(), vec![update("one")]);
        assert!(c.for_item(&item("unrelated")).is_empty());
        assert!(c.rows("a")[0].has_update());
        assert_eq!(c.for_item(&item("one")).len(), 1);
        c.retain_installed("a", &[]);
        assert!(c.rows("a").is_empty());
        c.clear();
        assert!(c.for_item(&item("one")).is_empty());
    }
}
