use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Local organization for one Epic account. Item keys come from Fab identities,
/// so renames, library syncs and missing items never discard these choices.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Organization {
    folders: Vec<String>,
    assignments: HashMap<String, String>,
    hidden: HashSet<String>,
}

#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub enum FolderFilter {
    #[default]
    All,
    Unfiled,
    Folder(String),
}

impl Organization {
    pub fn folders(&self) -> &[String] {
        &self.folders
    }
    pub fn folder(&self, item: &str) -> Option<&str> {
        self.assignments.get(item).map(String::as_str)
    }
    pub fn is_hidden(&self, item: &str) -> bool {
        self.hidden.contains(item)
    }
    pub fn set_hidden(&mut self, item: &str, hidden: bool) {
        if hidden {
            self.hidden.insert(item.into());
        } else {
            self.hidden.remove(item);
        }
    }
    pub fn matches(&self, item: &str, filter: &FolderFilter, show_hidden: bool) -> bool {
        (self.is_hidden(item) == show_hidden)
            && match filter {
                FolderFilter::All => true,
                FolderFilter::Unfiled => self.folder(item).is_none(),
                FolderFilter::Folder(folder) => self.folder(item) == Some(folder.as_str()),
            }
    }
    fn valid_name(&self, name: &str, previous: Option<&str>) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
            return Err("Use a folder name between 1 and 40 characters.".into());
        }
        if ["all items", "unfiled"]
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
        {
            return Err("Choose a different folder name.".into());
        }
        if self.folders.iter().any(|folder| {
            Some(folder.as_str()) != previous && folder.to_lowercase() == name.to_lowercase()
        }) {
            return Err("A folder with that name already exists.".into());
        }
        Ok(name.into())
    }
    pub fn create_folder(&mut self, name: &str) -> Result<String, String> {
        let name = self.valid_name(name, None)?;
        self.folders.push(name.clone());
        Ok(name)
    }
    pub fn rename_folder(&mut self, previous: &str, name: &str) -> Result<String, String> {
        let name = self.valid_name(name, Some(previous))?;
        let folder = self
            .folders
            .iter_mut()
            .find(|folder| folder.as_str() == previous)
            .ok_or("Folder no longer exists.")?;
        *folder = name.clone();
        for assigned in self
            .assignments
            .values_mut()
            .filter(|folder| folder.as_str() == previous)
        {
            *assigned = name.clone();
        }
        Ok(name)
    }
    /// Deleting a folder only unfiles its items, including hidden/offline items.
    pub fn delete_folder(&mut self, folder: &str) {
        self.folders.retain(|name| name != folder);
        self.assignments.retain(|_, assigned| assigned != folder);
    }
    pub fn move_item(&mut self, item: &str, folder: Option<&str>) -> Result<(), String> {
        if let Some(folder) = folder {
            if !self.folders.iter().any(|name| name == folder) {
                return Err("Folder no longer exists.".into());
            }
            self.assignments.insert(item.into(), folder.into());
        } else {
            self.assignments.remove(item);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_view_excludes_visible_items_and_restoring_returns_them_to_normal_view() {
        let mut org = Organization::default();
        org.create_folder("Tools").unwrap();
        for item in ["visible", "hidden"] {
            org.move_item(item, Some("Tools")).unwrap();
        }
        org.set_hidden("hidden", true);
        for filter in [FolderFilter::All, FolderFilter::Folder("Tools".into())] {
            assert!(org.matches("hidden", &filter, true));
            assert!(
                !org.matches("visible", &filter, true),
                "hidden-only view must exclude normal items"
            );
            assert!(!org.matches("hidden", &filter, false));
            assert!(org.matches("visible", &filter, false));
        }
        org.set_hidden("hidden", false);
        assert!(!org.matches("hidden", &FolderFilter::All, true));
        assert!(org.matches("hidden", &FolderFilter::All, false));
        assert!(!org.matches("unfiled-visible", &FolderFilter::Unfiled, true));
    }

    #[test]
    fn hidden_items_can_be_shown_and_restored_without_losing_their_folder() {
        let mut org = Organization::default();
        let name = org.create_folder(" Environments ").unwrap();
        org.move_item("namespace:item", Some(&name)).unwrap();
        org.set_hidden("namespace:item", true);
        let filter = FolderFilter::Folder(name);
        assert!(!org.matches("namespace:item", &FolderFilter::All, false));
        assert!(!org.matches("namespace:item", &filter, false));
        assert!(org.matches("namespace:item", &filter, true));
        assert!(!org.matches("namespace:item", &FolderFilter::Unfiled, true));
        org.set_hidden("namespace:item", false);
        assert!(org.matches("namespace:item", &filter, false));
    }

    #[test]
    fn rename_and_delete_preserve_items_even_when_they_are_hidden_or_not_in_current_library() {
        let mut org = Organization::default();
        org.create_folder("Tools").unwrap();
        org.move_item("absent-from-sync", Some("Tools")).unwrap();
        org.set_hidden("absent-from-sync", true);
        org.rename_folder("Tools", "Utilities").unwrap();
        assert_eq!(org.folder("absent-from-sync"), Some("Utilities"));
        org.delete_folder("Utilities");
        assert!(org.matches("absent-from-sync", &FolderFilter::Unfiled, true));
        assert!(org.is_hidden("absent-from-sync"));
        assert!(org.folders().is_empty());
    }

    #[test]
    fn invalid_names_and_destinations_do_not_change_assignments() {
        let mut org = Organization::default();
        org.create_folder("Tools").unwrap();
        org.create_folder("Art").unwrap();
        org.move_item("item", Some("Tools")).unwrap();
        for name in ["", "tools", "ALL ITEMS", "unfiled", "bad\nname"] {
            assert!(org.create_folder(name).is_err());
        }
        assert!(org.rename_folder("Tools", "ART").is_err());
        assert!(org.move_item("item", Some("Missing")).is_err());
        assert_eq!(org.folder("item"), Some("Tools"));
        org.rename_folder("Tools", "TOOLS").unwrap();
        org.move_item("item", None).unwrap();
        assert!(org.matches("item", &FolderFilter::Unfiled, false));
    }

    #[test]
    fn preferences_preserve_organization_per_account_and_migrate_old_files() {
        let mut prefs: crate::storage::Preferences = serde_json::from_str("{}").unwrap();
        assert!(prefs.library_organization.is_empty());
        let first = prefs
            .library_organization
            .entry("account-one".into())
            .or_default();
        first.create_folder("Tools").unwrap();
        first.move_item("same-item-id", Some("Tools")).unwrap();
        first.set_hidden("same-item-id", true);
        prefs
            .library_organization
            .entry("account-two".into())
            .or_default();
        let restored: crate::storage::Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert!(restored.library_organization["account-one"].is_hidden("same-item-id"));
        assert_eq!(
            restored.library_organization["account-one"].folder("same-item-id"),
            Some("Tools")
        );
        assert!(restored.library_organization["account-two"].matches(
            "same-item-id",
            &FolderFilter::Unfiled,
            false
        ));
    }
}
