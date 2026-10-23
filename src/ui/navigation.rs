use super::*;

pub(super) const START_PAGES: &[(Page, &str, &str)] = &[
    (Page::Workspace, "workspace", "Workspace"),
    (Page::Projects, "projects", "Projects"),
    (Page::Library, "library", "Library"),
    (Page::Downloads, "downloads", "Downloads"),
    (Page::VaultCache, "vault-cache", "Vault Cache"),
    (Page::FreeOffers, "free-offers", "Limited-Time Free"),
    (Page::Settings, "settings", "Settings"),
    (Page::Engines, "engines", "Engine versions"),
];
pub(super) fn project_order(prefs: &Preferences) -> ProjectSort {
    match prefs
        .project_sort
        .as_deref()
        .unwrap_or(&prefs.last_view.project_sort)
    {
        "name-asc" => ProjectSort::NameAscending,
        "name-desc" => ProjectSort::NameDescending,
        _ => ProjectSort::RecentlyOpened,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sort_choices_survive_restart_with_a_fixed_startup_page_and_order_the_items() {
        use engine_workspace::fab_listing::{self, Sort};
        let mut prefs = Preferences::default();
        prefs.startup_page = "workspace".into();
        prefs.project_sort = Some("name-desc".into());
        prefs.last_view.project_sort = "recent".into();
        prefs.library_sort = Sort::LatestAdded;
        let prefs: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert!(restored_view(&prefs).0 == Page::Workspace);
        assert!(project_order(&prefs) == ProjectSort::NameDescending);
        assert_eq!(prefs.library_sort.label(), "Latest added");
        let item = |title: &str, acquired: i64, updated: i64| {
            serde_json::from_value(serde_json::json!({
            "assetId":title, "assetNamespace":"test", "title":title, "acquiredAt":acquired, "listingUpdatedAt":updated
        })).unwrap()
        };
        let mut items = vec![item("Old", 1, 20), item("New", 2, 10)];
        fab_listing::sort(&mut items, prefs.library_sort);
        assert_eq!(items[0].title, "New");
        let mut prefs = prefs;
        prefs.library_sort = Sort::LatestUpdate;
        prefs.project_sort = Some("recent".into());
        let prefs: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert!(project_order(&prefs) == ProjectSort::RecentlyOpened);
        assert_eq!(prefs.library_sort.label(), "Latest update");
        fab_listing::sort(&mut items, prefs.library_sort);
        assert_eq!(items[0].title, "Old");
    }
    #[test]
    fn restart_restores_projects_pinned_filters_and_layout_from_saved_preferences() {
        let mut prefs = Preferences::default();
        prefs.last_view = storage::SavedView {
            page: "projects".into(),
            pinned_only: true,
            project_search: "example".into(),
            project_engine: "5.8".into(),
            project_sort: "name-desc".into(),
            project_list: true,
            ..Default::default()
        };
        let bytes = serde_json::to_vec(&prefs).unwrap();
        let reloaded: Preferences = serde_json::from_slice(&bytes).unwrap();
        let (page, view) = restored_view(&reloaded);
        assert!(page == Page::Projects);
        assert!(view == prefs.last_view);
        prefs.startup_page = "library".into();
        let (page, view) = restored_view(&prefs);
        assert!(page == Page::Library);
        assert!(!view.pinned_only && view.project_search.is_empty());
    }
    #[test]
    fn removed_category_hidden_page_and_old_preferences_have_safe_fallbacks() {
        let mut prefs: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs.startup_page, "last");
        assert!(restored_view(&prefs).0 == Page::Workspace);
        prefs.project_categories = vec!["Environments".into()];
        prefs.last_view.page = "projects".into();
        prefs.last_view.category = Some("Environments".into());
        assert_eq!(
            restored_view(&prefs).1.category.as_deref(),
            Some("Environments")
        );
        prefs.project_categories.clear();
        assert!(restored_view(&prefs).1.category.is_none());
        prefs.last_view.page = "vault-cache".into();
        prefs.show_vault_cache_button = false;
        assert!(restored_view(&prefs).0 == Page::Workspace);
    }
}

pub(super) fn restored_view(prefs: &Preferences) -> (Page, storage::SavedView) {
    let view = if prefs.startup_page == "last" {
        prefs.last_view.clone()
    } else {
        storage::SavedView {
            page: prefs.startup_page.clone(),
            ..Default::default()
        }
    };
    let mut page = START_PAGES
        .iter()
        .find(|(_, key, _)| *key == view.page)
        .map(|p| p.0)
        .unwrap_or(Page::Workspace);
    if (page == Page::VaultCache && !prefs.show_vault_cache_button)
        || (page == Page::FreeOffers && !prefs.show_free_offers_button)
    {
        page = Page::Workspace;
    }
    let mut view = view;
    if view
        .category
        .as_ref()
        .is_some_and(|name| !prefs.project_categories.contains(name))
    {
        view.category = None;
    }
    if view.pinned_only {
        view.category = None;
    }
    (page, view)
}

impl Workspace {
    pub(super) fn view_snapshot(&self) -> storage::SavedView {
        storage::SavedView {
            page: START_PAGES
                .iter()
                .find(|(page, _, _)| *page == self.page)
                .map(|p| p.1)
                .unwrap_or("workspace")
                .into(),
            pinned_only: self.pinned_only,
            category: self.category_filter.clone(),
            project_search: self.search.clone(),
            project_engine: self.filter.clone(),
            project_sort: match self.project_sort {
                ProjectSort::RecentlyOpened => "recent",
                ProjectSort::NameAscending => "name-asc",
                ProjectSort::NameDescending => "name-desc",
            }
            .into(),
            project_list: self.list_view,
        }
    }

    pub(super) fn remember_view(&mut self) {
        if self.demo
            || self.layout_probe.is_some()
            || std::env::args().any(|arg| arg.starts_with("--layout-probe="))
        {
            return;
        }
        let view = self.view_snapshot();
        if self.prefs.last_view != view || self.prefs.library_sort != self.library.sort {
            self.prefs.last_view = view;
            self.persist();
        }
    }

    pub(super) fn startup_preferences(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.heading("Startup page");
        let selected = if self.prefs.startup_page == "last" {
            "Remember last page and project view"
        } else {
            START_PAGES
                .iter()
                .find(|(_, key, _)| *key == self.prefs.startup_page)
                .map(|p| p.2)
                .unwrap_or("Workspace")
        };
        let before = self.prefs.startup_page.clone();
        AppComboBox::new(theme, "startup-page")
            .width(300.)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                buttons::selectable_value(
                    ui,
                    &mut self.prefs.startup_page,
                    "last".into(),
                    "Remember last page and project view",
                );
                for (_, key, label) in START_PAGES {
                    buttons::selectable_value(
                        ui,
                        &mut self.prefs.startup_page,
                        (*key).into(),
                        *label,
                    );
                }
            });
        ui.label("Remember your last page and project tab, including Pinned or a category, along with project filters, sorting and grid/list view. Or choose a fixed startup page.");
        if before != self.prefs.startup_page {
            self.persist();
        }
    }
}
