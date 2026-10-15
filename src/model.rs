use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct EngineVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}
impl EngineVersion {
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.strip_prefix("UE_").unwrap_or(raw);
        let mut numbers = raw.split('.');
        let major = numbers.next()?.parse().ok()?;
        let minor: String = numbers
            .next()?
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let patch: String = numbers
            .next()
            .unwrap_or("0")
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        Some(Self {
            major,
            minor: minor.parse().ok()?,
            patch: patch.parse().ok()?,
        })
    }
    pub fn branch(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }
}
impl std::fmt::Display for EngineVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Release {
    pub app: String,
    pub catalog_id: String,
    pub channel: String,
    pub platform: String,
    pub build: Option<String>,
    pub version: Option<EngineVersion>,
}
impl Release {
    pub fn key(&self) -> String {
        format!("{}:{}:{}", self.platform, self.channel, self.app)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChangeKind {
    NewBranch,
    Hotfix,
    BuildRevision,
    Rollback,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseChange {
    pub app: String,
    pub kind: ChangeKind,
    pub previous: Option<String>,
    pub current: String,
}

pub fn changes(previous: &[Release], current: &[Release]) -> Vec<ReleaseChange> {
    let old: BTreeMap<_, _> = previous.iter().map(|r| (r.key(), r)).collect();
    let mut result = Vec::new();
    for r in current {
        let Some(build) = &r.build else { continue };
        let prior = old.get(&r.key()).copied();
        // A catalog-only snapshot is not a baseline for an unseen authenticated build.
        let kind = match prior {
            None => ChangeKind::NewBranch,
            Some(p) if p.build.as_ref() == Some(build) => continue,
            Some(p) if p.build.is_none() => continue,
            Some(p) => match (&p.version, &r.version) {
                (Some(a), Some(b)) if b < a => ChangeKind::Rollback,
                (Some(a), Some(b)) if b > a && a.major == b.major && a.minor == b.minor => {
                    ChangeKind::Hotfix
                }
                (Some(a), Some(b)) if b > a => ChangeKind::NewBranch,
                _ => ChangeKind::BuildRevision,
            },
        };
        result.push(ReleaseChange {
            app: r.app.clone(),
            kind,
            previous: prior.and_then(|p| p.build.clone()),
            current: build.clone(),
        });
    }
    result
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub checked_at: u64,
    pub releases: Vec<Release>,
    pub catalog_count: usize,
    pub missing_builds: Vec<String>,
    pub authenticated: bool,
    pub notices: Vec<ReleaseChange>,
    #[serde(default)]
    pub seen: BTreeSet<String>,
}
impl Snapshot {
    pub fn verified(&self) -> bool {
        self.authenticated && !self.releases.is_empty() && self.missing_builds.is_empty()
    }
    pub fn merge_history(&mut self, previous: &Self) {
        self.seen = previous.seen.clone();
        self.notices = previous.notices.clone();
        if self.authenticated && previous.authenticated {
            for c in changes(&previous.releases, &self.releases) {
                let key = format!("Windows:Live:{}:{}", c.app, c.current);
                if self.seen.insert(key) {
                    self.notices.push(c);
                }
            }
        }
        for r in &self.releases {
            if let Some(b) = &r.build {
                self.seen.insert(format!("{}:{}", r.key(), b));
            }
        }
        if self.notices.len() > 100 {
            self.notices.drain(..self.notices.len() - 100);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installation {
    pub version: String,
    pub path: String,
    pub association: String,
}
/// Numeric comparison, independent of discovery order or the preferred engine.
pub fn latest_installation(engines: &[Installation]) -> Option<usize> {
    engines
        .iter()
        .enumerate()
        .filter_map(|(i, e)| EngineVersion::parse(&e.version).map(|v| (i, v)))
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(i, _)| i)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
    pub description: String,
    pub engine: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub last_opened: u64,
    #[serde(default)]
    pub artwork: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rel(app: &str, build: &str) -> Release {
        Release {
            app: app.into(),
            catalog_id: "id".into(),
            channel: "Live".into(),
            platform: "Windows".into(),
            build: Some(build.into()),
            version: EngineVersion::parse(build),
        }
    }
    #[test]
    fn numeric_versions_and_build_suffix() {
        assert!(
            EngineVersion::parse("5.10.0-123+++UE5").unwrap()
                > EngineVersion::parse("5.9.4-999").unwrap()
        );
        assert_eq!(EngineVersion::parse("UE_5.7").unwrap().patch, 0);
        assert!(EngineVersion::parse("garbage").is_none());
    }
    #[test]
    fn hotfix_same_app_is_detected() {
        assert_eq!(
            changes(&[rel("UE_5.7", "5.7.1-100")], &[rel("UE_5.7", "5.7.2-200")])[0].kind,
            ChangeKind::Hotfix
        );
    }
    #[test]
    fn new_branch_not_preinstalled() {
        assert_eq!(
            changes(&[rel("UE_5.7", "5.7.1")], &[rel("UE_5.8", "5.8.0")])[0].kind,
            ChangeKind::NewBranch
        );
    }
    #[test]
    fn rebuild_not_mislabeled_hotfix() {
        assert_eq!(
            changes(&[rel("UE_5.7", "5.7.1-100")], &[rel("UE_5.7", "5.7.1-200")])[0].kind,
            ChangeKind::BuildRevision
        );
    }
    #[test]
    fn rollback_classified() {
        assert_eq!(
            changes(&[rel("UE_5.7", "5.7.2")], &[rel("UE_5.7", "5.7.1")])[0].kind,
            ChangeKind::Rollback
        );
    }
    #[test]
    fn unchanged_does_not_notify() {
        assert!(changes(&[rel("UE_5.7", "5.7.2")], &[rel("UE_5.7", "5.7.2")]).is_empty());
    }
    #[test]
    fn first_run_and_restart_deduplicate() {
        let mut a = Snapshot {
            authenticated: true,
            releases: vec![rel("UE_5.7", "5.7.1")],
            ..Default::default()
        };
        a.merge_history(&Snapshot::default());
        assert!(a.notices.is_empty());
        let mut b = Snapshot {
            authenticated: true,
            releases: vec![rel("UE_5.7", "5.7.2")],
            ..Default::default()
        };
        b.merge_history(&a);
        assert_eq!(b.notices.len(), 1);
        let serialized = serde_json::to_string(&b).unwrap();
        let old = serde_json::from_str(&serialized).unwrap();
        b.merge_history(&old);
        assert_eq!(b.notices.len(), 1);
    }
    #[test]
    fn partial_and_anonymous_never_verified() {
        assert!(!Snapshot::default().verified());
        let s = Snapshot {
            authenticated: true,
            releases: vec![rel("UE_5.8", "5.8.0")],
            missing_builds: vec!["UE_5.9".into()],
            ..Default::default()
        };
        assert!(!s.verified());
    }
}

/// Unreal numeric associations select their branch; GUID associations match exactly.
pub fn project_engine(project: &Project, engines: &[Installation]) -> Option<usize> {
    let association = project.engine.trim();
    if association.is_empty() {
        return None;
    }
    engines.iter().position(|e| {
        e.association
            .trim_matches(['{', '}'])
            .eq_ignore_ascii_case(association.trim_matches(['{', '}']))
            || EngineVersion::parse(association)
                .zip(EngineVersion::parse(&e.version))
                .is_some_and(|(a, b)| a.major == b.major && a.minor == b.minor)
    })
}
