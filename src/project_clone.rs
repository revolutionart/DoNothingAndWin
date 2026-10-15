use crate::{installer, model::Project};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn clone_project(
    source: &Project,
    name: &str,
    destination: &Path,
    progress: impl Fn(usize, usize),
) -> Result<Project, String> {
    if name.is_empty()
        || name.len() > 100
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("Use letters, numbers and underscores for the new project name.".into());
    }
    installer::relative(&format!("{name}.uproject"))?;
    installer::no_links(Path::new(&source.path))?;
    let descriptor = fs::canonicalize(&source.path).map_err(|e| e.to_string())?;
    let root = descriptor.parent().ok_or("Project has no parent folder")?;
    let _: serde_json::Value =
        serde_json::from_slice(&fs::read(&descriptor).map_err(|e| e.to_string())?)
            .map_err(|_| "Invalid source project")?;
    if !destination.is_absolute() {
        return Err("Choose an absolute destination folder.".into());
    }
    installer::no_links(destination)?;
    if destination.exists() {
        return Err("Destination already exists. Choose a new folder.".into());
    }
    let parent = fs::canonicalize(destination.parent().ok_or("Invalid destination")?)
        .map_err(|_| "Destination parent folder must exist")?;
    let destination = parent.join(
        destination
            .file_name()
            .ok_or("Invalid destination folder")?,
    );
    if parent.starts_with(root) || root.starts_with(&destination) {
        return Err("Clone destination must be outside the original project.".into());
    }
    fn collect(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, u64)>) -> Result<(), String> {
        for e in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let path = e.map_err(|e| e.to_string())?.path();
            installer::no_links(&path)?;
            if path.is_dir() {
                collect(root, &path, out)?;
            } else if path.is_file() {
                out.push((
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::metadata(&path).map_err(|e| e.to_string())?.len(),
                ));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    let old = descriptor.file_name().ok_or("Missing project filename")?;
    let renamed = PathBuf::from(format!("{name}.uproject"));
    if renamed.file_name() != Some(old)
        && files.iter().any(|(p, _)| {
            p.to_string_lossy()
                .eq_ignore_ascii_case(&renamed.to_string_lossy())
        })
    {
        return Err("A file already uses the new project name.".into());
    }
    let size: u64 = files.iter().map(|(_, s)| s).sum();
    if fs2::available_space(&parent).map_err(|e| e.to_string())? < size + 16 * 1024 * 1024 {
        return Err("Not enough free space to clone this project.".into());
    }
    let stage = parent.join(format!(
        ".engine-workspace-clone-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        for (i, (relative, _)) in files.iter().enumerate() {
            let input = root.join(relative);
            installer::no_links(&input)?;
            let before = fs::metadata(&input).map_err(|e| e.to_string())?;
            let output = stage.join(if relative == Path::new(old) {
                &renamed
            } else {
                relative
            });
            fs::create_dir_all(output.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(&input, &output).map_err(|e| e.to_string())?;
            let after = fs::metadata(&input).map_err(|e| e.to_string())?;
            if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
                return Err("Source changed while copying. Close the editor and retry.".into());
            }
            progress(i + 1, files.len());
        }
        installer::no_links(&destination)?;
        if destination.exists() {
            return Err("Destination appeared during cloning; nothing overwritten.".into());
        }
        fs::rename(&stage, &destination).map_err(|e| e.to_string())?;
        Ok(Project {
            name: name.into(),
            path: destination.join(renamed).to_string_lossy().into_owned(),
            engine: source.engine.clone(),
            description: source.description.clone(),
            pinned: false,
            last_opened: 0,
            artwork: source
                .artwork
                .as_ref()
                .and_then(|a| Path::new(a).strip_prefix(root).ok())
                .map(|p| destination.join(p).to_string_lossy().into_owned()),
        })
    })();
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clone_preserves_original_and_renames_only_descriptor() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Original");
        fs::create_dir_all(root.join("Content")).unwrap();
        fs::write(
            root.join("Original.uproject"),
            br#"{"EngineAssociation":"5.7","Modules":[{"Name":"Original"}]}"#,
        )
        .unwrap();
        fs::write(root.join("Content/test.uasset"), b"unchanged").unwrap();
        let p = Project {
            name: "Original".into(),
            path: root
                .join("Original.uproject")
                .to_string_lossy()
                .into_owned(),
            engine: "5.7".into(),
            description: String::new(),
            pinned: true,
            last_opened: 5,
            artwork: None,
        };
        let old = fs::read(&p.path).unwrap();
        let dest = d.path().join("Copy");
        let copy = clone_project(&p, "Renamed", &dest, |_, _| {}).unwrap();
        assert_eq!(fs::read(&p.path).unwrap(), old);
        assert_eq!(fs::read(&copy.path).unwrap(), old);
        assert_eq!(
            fs::read(dest.join("Content/test.uasset")).unwrap(),
            b"unchanged"
        );
        assert!(!dest.join("Original.uproject").exists());
        assert!(clone_project(&p, "Renamed", &dest, |_, _| {}).is_err());
        assert!(clone_project(&p, "Bad", &root.join("Nested"), |_, _| {}).is_err());
        assert!(clone_project(&p, "../Bad", &d.path().join("Bad"), |_, _| {}).is_err());
    }
}
