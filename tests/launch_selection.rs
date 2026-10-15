use engine_workspace::model::{Installation, latest_installation};

fn engine(version: &str) -> Installation {
    Installation {
        version: version.into(),
        path: format!("F:/Engines/{version}"),
        association: version.into(),
    }
}

#[test]
fn selects_newest_installed_hotfix_in_any_order() {
    let engines = vec![
        engine("5.8.0"),
        engine("5.7.4"),
        engine("5.8.3"),
        engine("5.8.2"),
    ];
    assert_eq!(latest_installation(&engines), Some(2));
    let engines = vec![engine("5.9.9"), engine("5.10.1"), engine("5.8.12")];
    assert_eq!(latest_installation(&engines), Some(1));
}

#[test]
fn no_engines_or_unparseable_versions_do_not_choose_arbitrarily() {
    assert_eq!(latest_installation(&[]), None);
    assert_eq!(latest_installation(&[engine("custom")]), None);
    assert_eq!(
        latest_installation(&[engine("5.8.3"), engine("5.8.3")]),
        Some(0)
    );
}

#[test]
fn project_associations_distinguish_missing_branches_and_custom_builds() {
    use engine_workspace::model::{Project, project_engine};
    let mut engines = vec![engine("5.8.3")];
    let mut project = Project {
        engine: "5.8".into(),
        name: String::new(),
        path: String::new(),
        description: String::new(),
        pinned: false,
        last_opened: 0,
        artwork: None,
    };
    assert_eq!(project_engine(&project, &engines), Some(0));
    project.engine = "5.7".into();
    assert_eq!(project_engine(&project, &engines), None);
    project.engine = "{ABC-123}".into();
    assert_eq!(project_engine(&project, &engines), None);
    engines[0].association = "abc-123".into();
    assert_eq!(project_engine(&project, &engines), Some(0));
    project.engine.clear();
    assert_eq!(project_engine(&project, &engines), None);
}
