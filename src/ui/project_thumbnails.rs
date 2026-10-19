use super::*;
use std::time::SystemTime;

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    artwork: String,
    modified: SystemTime,
    length: u64,
}
impl Stamp {
    fn read(artwork: String) -> Option<Self> {
        let metadata = std::fs::metadata(&artwork).ok()?;
        Some(Self {
            artwork,
            modified: metadata.modified().ok()?,
            length: metadata.len(),
        })
    }
}
struct Update {
    project: String,
    stamp: Option<Stamp>,
    image: Option<ColorImage>,
}
struct Scan {
    revision: u64,
    updates: Vec<Update>,
}
#[derive(Default)]
pub(super) struct Thumbnails {
    known: HashMap<String, Stamp>,
    pending: Option<Receiver<Scan>>,
    next_scan: Option<Instant>,
    revision: u64,
}
impl Thumbnails {
    pub fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.next_scan = None;
    }
}

// All directory/metadata reads and image decoding happen on the worker.
fn scan(projects: &[String], known: &HashMap<String, Stamp>) -> Vec<Update> {
    let mut updates = Vec::new();
    for project in projects {
        let Some(artwork) = platform::project_thumbnail(Path::new(project)) else {
            // An inaccessible project directory is not evidence that its artwork was deleted.
            if Path::new(project)
                .parent()
                .is_some_and(|root| std::fs::read_dir(root).is_ok())
            {
                updates.push(Update {
                    project: project.clone(),
                    stamp: None,
                    image: None,
                });
            }
            continue;
        };
        let Some(stamp) = Stamp::read(artwork) else {
            continue;
        };
        if known.get(project) == Some(&stamp) {
            // Also restores the selected path after a project rescan.
            updates.push(Update {
                project: project.clone(),
                stamp: Some(stamp),
                image: None,
            });
            continue;
        }
        let Ok(image) = image::open(&stamp.artwork) else {
            continue;
        };
        let image = image.thumbnail(820, 360).to_rgba8();
        // Don't accept a file that changed while it was being decoded. Failed reads
        // retain the previous texture and stamp so the next scan tries again.
        if Stamp::read(stamp.artwork.clone()).as_ref() != Some(&stamp) {
            continue;
        }
        updates.push(Update {
            project: project.clone(),
            stamp: Some(stamp),
            image: Some(ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize],
                image.as_raw(),
            )),
        });
    }
    updates
}

impl Workspace {
    pub(super) fn poll_project_thumbnails(&mut self, ctx: &Context) {
        if self.demo {
            return;
        }
        if let Some(rx) = &self.project_thumbnails.pending {
            match rx.try_recv() {
                Ok(result) => {
                    self.project_thumbnails.pending = None;
                    if result.revision == self.project_thumbnails.revision {
                        let old_artwork: Vec<_> = self
                            .project_thumbnails
                            .known
                            .values()
                            .map(|stamp| stamp.artwork.clone())
                            .collect();
                        self.project_thumbnails
                            .known
                            .retain(|path, _| self.projects.iter().any(|p| &p.path == path));
                        for update in result.updates {
                            let Some(project) =
                                self.projects.iter_mut().find(|p| p.path == update.project)
                            else {
                                continue;
                            };
                            project.artwork = update.stamp.as_ref().map(|s| s.artwork.clone());
                            if let Some(stamp) = update.stamp {
                                if let Some(image) = update.image {
                                    if let Some(texture) = self.textures.get_mut(&stamp.artwork) {
                                        texture.set(image, TextureOptions::LINEAR);
                                    } else {
                                        self.textures.insert(
                                            stamp.artwork.clone(),
                                            ctx.load_texture(
                                                &stamp.artwork,
                                                image,
                                                TextureOptions::LINEAR,
                                            ),
                                        );
                                    }
                                }
                                self.project_thumbnails.known.insert(update.project, stamp);
                            } else {
                                self.project_thumbnails.known.remove(&update.project);
                            }
                        }
                        for old in old_artwork {
                            if !self
                                .projects
                                .iter()
                                .any(|p| p.artwork.as_ref() == Some(&old))
                            {
                                self.textures.remove(&old);
                            }
                        }
                        self.project_thumbnails.next_scan = Some(Instant::now() + REFRESH_INTERVAL);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.project_thumbnails.pending = None;
                    self.project_thumbnails.next_scan = Some(Instant::now() + REFRESH_INTERVAL);
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.project_thumbnails.pending.is_none()
            && self
                .project_thumbnails
                .next_scan
                .is_none_or(|next| Instant::now() >= next)
        {
            let projects = self
                .projects
                .iter()
                .map(|p| p.path.clone())
                .collect::<Vec<_>>();
            let known = self.project_thumbnails.known.clone();
            let revision = self.project_thumbnails.revision;
            let (tx, rx) = mpsc::channel();
            self.project_thumbnails.pending = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Scan {
                    revision,
                    updates: scan(&projects, &known),
                });
                ctx.request_repaint();
            });
        }
        ctx.request_repaint_after(REFRESH_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screenshot(path: &Path, color: [u8; 4], seconds: u64) {
        image::RgbaImage::from_pixel(16, 9, image::Rgba(color))
            .save(path)
            .unwrap();
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(time))
            .unwrap();
    }
    #[test]
    fn overwritten_screenshot_refreshes_pixels_and_unchanged_images_are_not_decoded() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("Saved")).unwrap();
        let project = temp
            .path()
            .join("Example.uproject")
            .to_string_lossy()
            .into_owned();
        let path = temp.path().join("Saved/AutoScreenshot.png");
        let projects = vec![project.clone()];
        screenshot(&path, [255, 0, 0, 255], 100);
        let mut first = scan(&projects, &HashMap::new());
        let first = first.pop().unwrap();
        assert_eq!(first.image.unwrap().pixels[0], Color32::RED);
        let known = HashMap::from([(project, first.stamp.unwrap())]);
        assert!(scan(&projects, &known)[0].image.is_none());
        screenshot(&path, [0, 255, 0, 255], 101);
        let updated = scan(&projects, &known);
        assert_eq!(updated[0].image.as_ref().unwrap().pixels[0], Color32::GREEN);
        assert_eq!(
            updated[0].stamp.as_ref().unwrap().length,
            known.values().next().unwrap().length
        );
    }
    #[test]
    fn incomplete_write_is_retried_and_new_or_deleted_artwork_is_detected() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("Saved")).unwrap();
        let project = temp
            .path()
            .join("Example.uproject")
            .to_string_lossy()
            .into_owned();
        let projects = vec![project.clone()];
        let path = temp.path().join("Saved/AutoScreenshot.png");
        assert!(scan(&projects, &HashMap::new())[0].stamp.is_none());
        screenshot(&path, [255, 0, 0, 255], 100);
        let stamp = scan(&projects, &HashMap::new())
            .pop()
            .unwrap()
            .stamp
            .unwrap();
        let known = HashMap::from([(project, stamp)]);
        std::fs::write(&path, b"partial PNG").unwrap();
        assert!(scan(&projects, &known).is_empty());
        screenshot(&path, [0, 255, 0, 255], 101);
        assert!(scan(&projects, &known)[0].image.is_some());
        std::fs::remove_file(&path).unwrap();
        assert!(scan(&projects, &known)[0].stamp.is_none());
    }
    #[test]
    fn custom_root_image_keeps_priority_and_deletion_falls_back_to_screenshot() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("Saved")).unwrap();
        let projects = vec![
            temp.path()
                .join("Example.uproject")
                .to_string_lossy()
                .into_owned(),
        ];
        let custom = temp.path().join("Example.png");
        screenshot(
            &temp.path().join("Saved/AutoScreenshot.png"),
            [0, 255, 0, 255],
            101,
        );
        screenshot(&custom, [255, 0, 0, 255], 100);
        assert_eq!(
            scan(&projects, &HashMap::new())[0]
                .image
                .as_ref()
                .unwrap()
                .pixels[0],
            Color32::RED
        );
        std::fs::remove_file(custom).unwrap();
        assert_eq!(
            scan(&projects, &HashMap::new())[0]
                .image
                .as_ref()
                .unwrap()
                .pixels[0],
            Color32::GREEN
        );
    }
}
