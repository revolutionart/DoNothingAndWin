use super::*;
use engine_workspace::vault_health::{self as health, Gate, Health, Record, Report, Selection};
use std::path::PathBuf;

enum Event {
    Progress(String),
    Finished(Result<Vec<Record>, String>),
}
fn current_result(
    gate: Option<&Gate>,
    result: Result<Vec<Record>, String>,
) -> Result<Vec<Record>, String> {
    if gate.is_some_and(Gate::cancelled) {
        Err("Cache verification stopped.".into())
    } else {
        result
    }
}
#[derive(Default)]
pub(super) struct State {
    report: Report,
    roots: Vec<String>,
    rx: Option<Receiver<Event>>,
    gate: Option<Gate>,
    startup_checked: bool,
    automatic: bool,
    selection: Option<Selection>,
    pending_manual: Option<Selection>,
    pub status: String,
    progress: health::Progress,
    pause_reason: String,
    details_open: bool,
    paused: bool,
    retry_after_work: bool,
    item_records: HashMap<String, Vec<String>>,
}
impl State {
    fn invalidate_changed(&mut self, paths: &[PathBuf]) -> bool {
        self.invalidate_paths(
            paths,
            "These cached files are being changed. Verify again after the operation finishes.",
        )
    }
    fn invalidate_paths(&mut self, paths: &[PathBuf], detail: &str) -> bool {
        let paths: Vec<_> = paths
            .iter()
            .filter(|path| {
                self.roots
                    .iter()
                    .any(|root| overlaps(path, Path::new(root)))
            })
            .collect();
        if paths.is_empty() {
            return false;
        }
        let mut changed = self.report.finished_all_at != 0;
        for record in self.report.records.values_mut() {
            if paths.iter().any(|path| overlaps(path, &record.path)) {
                changed |= record.health != Health::Unknown || record.detail != detail;
                record.health = Health::Unknown;
                record.detail = detail.into();
            }
        }
        self.report.finished_all_at = 0;
        changed
    }
    fn invalidate_failed_check(&mut self, error: &str) -> bool {
        let paths = match &self.selection {
            Some(Selection::Entry(path)) => vec![path.clone()],
            Some(Selection::Item(item)) => {
                self.for_item(item).iter().map(|r| r.path.clone()).collect()
            }
            Some(Selection::All) if !self.progress.entry.as_os_str().is_empty() => {
                vec![self.progress.entry.clone()]
            }
            _ => Vec::new(),
        };
        self.invalidate_paths(
            &paths,
            &format!("Could not complete the latest check: {error}"),
        )
    }
    pub fn load() -> Self {
        let mut state = Self::default();
        state.report = health::load();
        state
    }
    pub fn busy(&self) -> bool {
        self.rx.is_some() || self.retry_after_work || self.pending_manual.is_some()
    }
    pub fn cancel(&mut self) {
        self.retry_after_work = false;
        self.pending_manual = None;
        if let Some(gate) = &self.gate {
            gate.cancel();
        }
    }
    fn prioritize(&mut self, selection: Selection) {
        // Stop the old reader before starting the selected check. Never hash in parallel.
        self.cancel();
        self.pending_manual = Some(selection);
        self.status = "Selected verification queued; waiting for the current check to stop.".into();
    }
    fn take_pending_manual(&mut self, work_busy: bool) -> Option<Selection> {
        if self.rx.is_some() || work_busy {
            return None;
        }
        let selection = self.pending_manual.take()?;
        self.retry_after_work = false;
        Some(selection)
    }
    fn heading(&self) -> String {
        let state = if self.pending_manual.is_some() {
            "Queued"
        } else if !self.busy() {
            if self.status == "Cache verification stopped." {
                "Stopped"
            } else {
                "Finished"
            }
        } else if self.gate.as_ref().is_some_and(Gate::cancelled) && !self.retry_after_work {
            "Stopping"
        } else if self.paused || self.retry_after_work {
            "Paused"
        } else {
            "Checking"
        };
        let item = if let Some(Selection::Item(item)) = &self.selection {
            item.title.clone()
        } else if !self.progress.entry.as_os_str().is_empty() {
            path_title(&self.progress.entry)
        } else {
            self.selection
                .as_ref()
                .map(selection_title)
                .unwrap_or_else(|| "All cache locations".into())
        };
        format!("Cache verification · {state} · {item}")
    }
    fn detail(&self) -> String {
        if !self.busy() {
            return self.status.clone();
        }
        if let Some(next) = &self.pending_manual {
            let reason = if !self.paused && !self.retry_after_work {
                "Stopping the previous verification"
            } else {
                &self.pause_reason
            };
            return format!("Next: {} · {reason}", selection_title(next));
        }
        if self.gate.as_ref().is_some_and(Gate::cancelled) && !self.retry_after_work {
            return "Stopping the file reader…".into();
        }
        if self.paused || self.retry_after_work {
            return self.pause_reason.clone();
        }
        let p = &self.progress;
        let count = if p.files_total > 0 {
            format!("{} / {} files checked", p.files_completed, p.files_total)
        } else {
            p.phase.clone()
        };
        format!(
            "{count} · {} / {} folders inspected",
            p.entries_completed, p.entries_total
        )
    }
    fn fraction(&self) -> f32 {
        let p = &self.progress;
        if p.files_total > 0 {
            let file_fraction = if p.file_total > 0 {
                (p.file_bytes as f32 / p.file_total as f32).min(1.)
            } else {
                0.
            };
            ((p.files_completed as f32 + file_fraction) / p.files_total as f32).min(1.)
        } else {
            0.
        }
    }
    pub fn forget(&mut self, path: &Path) {
        self.report.records.remove(&health::key(path));
        self.report.finished_all_at = 0;
        health::save(&self.report);
    }
    fn records(&self) -> impl Iterator<Item = &Record> {
        self.report.records.values().filter(|record| {
            self.roots
                .iter()
                .any(|root| health::key(Path::new(root)) == health::key(&record.root))
        })
    }
    pub fn for_path(&self, path: &Path) -> Option<&Record> {
        self.report
            .records
            .get(&health::key(path))
            .filter(|record| self.active(record))
    }
    fn active(&self, record: &Record) -> bool {
        self.roots
            .iter()
            .any(|root| health::key(Path::new(root)) == health::key(&record.root))
    }
    fn relink(&mut self, items: &[engine_workspace::fab::Item]) {
        let mut links = HashMap::<String, Vec<String>>::new();
        for r in self.records() {
            let entry = engine_workspace::vault_cache::Entry {
                root: r.root.clone(),
                path: r.path.clone(),
                title: r.title.clone(),
                bytes: 0,
                identities: r.identities.clone(),
                removable: false,
                incomplete: false,
            };
            if let Some(item) = engine_workspace::vault_cache::match_item(&entry, items) {
                links
                    .entry(item.key())
                    .or_default()
                    .push(health::key(&r.path));
            }
        }
        self.item_records = links;
    }
    fn for_item(&self, item: &engine_workspace::fab::Item) -> Vec<&Record> {
        self.item_records
            .get(&item.key())
            .into_iter()
            .flatten()
            .filter_map(|key| self.report.records.get(key))
            .filter(|record| self.active(record))
            .collect()
    }
}
fn overlaps(a: &Path, b: &Path) -> bool {
    engine_workspace::discovery::inside(a, b) || engine_workspace::discovery::inside(b, a)
}
fn path_title(path: &Path) -> String {
    if path.ends_with("EngineWorkspace/Chunks") {
        "Shared download chunks".into()
    } else {
        path.file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
}
fn selection_title(selection: &Selection) -> String {
    match selection {
        Selection::All => "All cache locations".into(),
        Selection::Entry(path) => path_title(path),
        Selection::Item(item) => item.title.clone(),
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn download_changes_only_invalidate_the_affected_entry_in_its_own_cache() {
        let mut state = State::default();
        state.roots = vec!["C:/Cache".into(), "D:/Cache".into()];
        for (path, health) in [
            ("C:/Cache/PCGEx", Health::Healthy),
            ("C:/Cache/FlexPool", Health::Healthy),
            ("D:/Cache/FlexPool", Health::Healthy),
            ("C:/Cache/FabLibrary/PCGEx", Health::MetadataOnly),
            ("C:/Cache/EngineWorkspace/Chunks", Health::Healthy),
        ] {
            let path = PathBuf::from(path);
            state.report.records.insert(
                health::key(&path),
                Record {
                    root: path
                        .ancestors()
                        .find(|p| p.file_name().is_some_and(|n| n == "Cache"))
                        .unwrap()
                        .into(),
                    path,
                    title: String::new(),
                    identities: vec![],
                    health,
                    detail: "Verified before download".into(),
                    checked_at: 42,
                    files: 5,
                },
            );
        }
        assert!(state.invalidate_changed(&["C:/Cache/FlexPool".into()]));
        let check = |s: &State, path: &str| s.report.records[&health::key(Path::new(path))].health;
        assert_eq!(check(&state, "C:/Cache/PCGEx"), Health::Healthy);
        assert_eq!(check(&state, "C:/Cache/FlexPool"), Health::Unknown);
        assert_eq!(check(&state, "D:/Cache/FlexPool"), Health::Healthy);
        assert_eq!(
            check(&state, "C:/Cache/FabLibrary/PCGEx"),
            Health::MetadataOnly
        );
        assert_eq!(
            check(&state, "C:/Cache/EngineWorkspace/Chunks"),
            Health::Healthy
        );
        state.invalidate_changed(&["C:/Cache/EngineWorkspace/Chunks/new-chunk".into()]);
        assert_eq!(
            check(&state, "C:/Cache/EngineWorkspace/Chunks"),
            Health::Unknown
        );
        assert_eq!(check(&state, "C:/Cache/PCGEx"), Health::Healthy);
        assert!(!state.invalidate_changed(&["C:/Projects/Unrelated".into()]));
        assert!(
            !state.invalidate_changed(&["C:/Cache/PCG".into()]),
            "A sibling with the same prefix must not invalidate PCGEx"
        );
        state.selection = Some(Selection::Entry("C:/Cache/PCGEx".into()));
        assert!(state.invalidate_failed_check("Could not read this item"));
        assert_eq!(check(&state, "C:/Cache/PCGEx"), Health::Unknown);
        assert_eq!(check(&state, "D:/Cache/FlexPool"), Health::Healthy);
        assert_eq!(
            check(&state, "C:/Cache/FabLibrary/PCGEx"),
            Health::MetadataOnly
        );
    }
    #[test]
    fn status_identifies_running_paused_queued_stopping_and_finished_checks() {
        let mut state = State::default();
        let (_tx, rx) = mpsc::channel();
        state.rx = Some(rx);
        state.gate = Some(Gate::default());
        state.progress = health::Progress {
            entry: "C:/Cache/EngineWorkspace/Chunks".into(),
            files_completed: 4,
            files_total: 10,
            file_bytes: 5,
            file_total: 10,
            entries_completed: 1,
            entries_total: 3,
            ..Default::default()
        };
        assert!(
            state
                .heading()
                .contains("Checking · Shared download chunks")
        );
        assert!(state.detail().contains("4 / 10 files checked"));
        assert!((state.fraction() - 0.45).abs() < 0.001);
        state.paused = true;
        state.pause_reason = "Waiting for the project backup to finish.".into();
        assert!(state.heading().contains("Paused"));
        assert!(state.detail().contains("project backup"));
        state.prioritize(Selection::Entry("C:/Cache/PCGEx".into()));
        assert!(state.heading().contains("Queued"));
        assert!(state.detail().contains("Next: PCGEx"));
        state.cancel();
        assert!(state.heading().contains("Stopping"));
        assert!(state.detail().contains("Stopping"));
        state.rx = None;
        state.status = "Cache checked: 1 healthy.".into();
        assert!(state.heading().contains("Finished"));
        assert_eq!(state.detail(), "Cache checked: 1 healthy.");
    }
    #[test]
    fn manual_selection_waits_for_the_old_worker_and_cache_operations() {
        let mut state = State::default();
        let gate = Gate::default();
        let (_tx, rx) = mpsc::channel();
        state.gate = Some(gate.clone());
        state.rx = Some(rx);
        state.automatic = true;
        state.retry_after_work = true;
        state.prioritize(Selection::Entry("C:/Cache/Selected".into()));
        assert!(gate.cancelled());
        assert!(state.busy());
        assert!(!state.retry_after_work);
        assert!(state.take_pending_manual(false).is_none());
        state.rx = None;
        assert!(state.take_pending_manual(true).is_none());
        assert!(
            matches!(state.take_pending_manual(false), Some(Selection::Entry(path)) if path == Path::new("C:/Cache/Selected"))
        );
        assert!(!state.busy());
    }
    #[test]
    fn stop_clears_a_pending_manual_check_without_losing_operation_retries() {
        let mut state = State::default();
        state.retry_after_work = true;
        assert!(state.take_pending_manual(false).is_none());
        assert!(state.retry_after_work);
        state.prioritize(Selection::All);
        state.cancel();
        assert!(state.take_pending_manual(false).is_none());
        assert!(!state.busy());
    }
    #[test]
    fn cache_mutation_discards_even_a_success_already_queued_by_the_worker() {
        let gate = Gate::default();
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(Vec::new())).unwrap();
        gate.cancel();
        assert!(current_result(Some(&gate), rx.recv().unwrap()).is_err());
        assert!(current_result(Some(&Gate::default()), Ok(Vec::new())).is_ok());
    }
    #[test]
    fn cached_links_keep_duplicate_titles_neutral_and_match_exact_identity_once() {
        let item = |id: &str| {
            serde_json::from_value(serde_json::json!({
                "assetId":id,"assetNamespace":"test","title":"Same title"
            }))
            .unwrap()
        };
        let items = vec![item("a"), item("b")];
        let mut state = State::default();
        state.roots = vec!["C:/Cache".into()];
        let record = Record {
            root: "C:/Cache".into(),
            path: "C:/Cache/Package".into(),
            title: "Same title".into(),
            identities: vec![],
            health: Health::Healthy,
            detail: "Verified".into(),
            checked_at: 1,
            files: 1,
        };
        state
            .report
            .records
            .insert(health::key(&record.path), record);
        state.relink(&items);
        assert!(state.for_item(&items[0]).is_empty());
        assert!(state.for_item(&items[1]).is_empty());
        state
            .report
            .records
            .values_mut()
            .next()
            .unwrap()
            .identities
            .push("a".into());
        state.relink(&items);
        assert_eq!(state.for_item(&items[0]).len(), 1);
        assert!(state.for_item(&items[1]).is_empty());
        state.roots = vec!["D:/Another cache".into()];
        assert!(state.for_item(&items[0]).is_empty());
    }
}

fn label(health: Health) -> &'static str {
    match health {
        Health::Healthy => "Healthy",
        Health::Redownload => "Needs redownload",
        Health::Unknown => "Not verified",
        Health::MetadataOnly => "Metadata only",
    }
}
fn tint(theme: &Theme, health: Health) -> [u8; 3] {
    match health {
        Health::Healthy => theme.success,
        Health::Redownload => theme.warning,
        Health::Unknown | Health::MetadataOnly => theme.muted,
    }
}
fn tooltip(record: &Record) -> String {
    let date = time::OffsetDateTime::from_unix_timestamp(record.checked_at as i64)
        .ok()
        .map(|t| format!("{} {:02}:{:02} UTC", t.date(), t.hour(), t.minute()))
        .unwrap_or_default();
    format!(
        "{}\n{}\n{}\nChecked {date}",
        label(record.health),
        record.detail,
        record.path.display()
    )
}

impl Workspace {
    pub(super) fn start_cache_verification(
        &mut self,
        selection: Selection,
        automatic: bool,
        ctx: &Context,
    ) {
        if self.demo || std::env::args().any(|a| a.starts_with("--layout-probe=")) {
            return;
        }
        if self.cache_health.busy() {
            if !automatic {
                self.cache_health.prioritize(selection);
                ctx.request_repaint();
            }
            return;
        }
        self.cache_health.roots = self.prefs.vault_cache_roots();
        self.cache_health.automatic = automatic;
        self.cache_health.selection = Some(selection.clone());
        self.cache_health.status = "Checking cached files…".into();
        self.cache_health.progress = health::Progress {
            phase: "Finding cached downloads".into(),
            ..Default::default()
        };
        let roots = self.cache_health.roots.clone();
        let gate = Gate::default();
        gate.pause(self.cache_health.paused);
        self.cache_health.gate = Some(gate.clone());
        let (tx, rx) = mpsc::channel();
        self.cache_health.rx = Some(rx);
        let session = if automatic {
            None
        } else {
            self.session.clone()
        };
        let items = self.library.cache_items().to_vec();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut last = Instant::now() - Duration::from_secs(1);
            let progress = |path: &Path| {
                if last.elapsed() >= Duration::from_millis(500) {
                    let _ = tx.send(Event::Progress(path.display().to_string()));
                    ctx.request_repaint();
                    last = Instant::now();
                }
            };
            let result = if let Some(session) = session {
                health::verify_roots_with_catalog(
                    &roots, &selection, &gate, progress, &session, &items,
                )
            } else {
                health::verify_roots(&roots, &selection, &gate, progress)
            };
            let _ = tx.send(Event::Finished(result));
            ctx.request_repaint();
        });
    }
    pub(super) fn poll_cache_health(&mut self, ctx: &Context) {
        if self.demo || std::env::args().any(|a| a.starts_with("--layout-probe=")) {
            return;
        }
        let roots = self.prefs.vault_cache_roots();
        if self.cache_health.roots != roots {
            self.cache_health.cancel();
            if !self.cache_health.busy() {
                self.cache_health.roots = roots.clone();
                self.cache_health.startup_checked = false;
                self.rebuild_cache_health_links();
            }
        }
        let transfer_busy = self.transfer.busy()
            || self.library_work_pending()
            || self.plugin_work.busy()
            || self.backup.busy()
            || self.cloning.busy()
            || self.launcher_updates.blocks_work()
            || self.vault.removing();
        let changed_paths: Vec<_> = health::take_changed_paths()
            .into_iter()
            .filter(|path| roots.iter().any(|root| overlaps(path, Path::new(root))))
            .collect();
        if !changed_paths.is_empty() {
            if self.cache_health.invalidate_changed(&changed_paths) {
                health::save(&self.cache_health.report);
            }
            if let Some(gate) = &self.cache_health.gate {
                if !gate.cancelled() {
                    gate.cancel();
                    self.cache_health.retry_after_work = true;
                }
            }
        }
        self.cache_health.pause_reason = if self.transfer.busy() {
            "Waiting for the engine download or installation to finish."
        } else if self.library_work_pending() {
            "Waiting for the Library download or project operation to finish."
        } else if self.plugin_work.busy() {
            "Waiting for the plugin operation to finish."
        } else if self.backup.busy() {
            "Waiting for the project backup to finish."
        } else if self.cloning.busy() {
            "Waiting for the project copy to finish."
        } else if self.launcher_updates.blocks_work() {
            "Waiting for the launcher update to finish."
        } else if self.vault.removing() {
            "Waiting for the Vault Cache scan or removal to finish."
        } else {
            ""
        }
        .into();
        self.cache_health.paused = transfer_busy;
        if let Some(gate) = &self.cache_health.gate {
            gate.pause(self.cache_health.paused);
            self.cache_health.progress = gate.progress();
        }
        if self.cache_health.automatic
            && !self.prefs.verify_cache_on_startup
            && self.cache_health.pending_manual.is_none()
        {
            self.cache_health.cancel();
        }
        loop {
            let event = match self.cache_health.rx.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(e)) => e,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.cache_health.status = "Cache verification stopped.".into();
                    self.cache_health.rx = None;
                    self.cache_health.gate = None;
                    break;
                }
                _ => break,
            };
            match event {
                Event::Progress(path) => {
                    if self.cache_health.pending_manual.is_none() {
                        self.cache_health.status = format!("Checking {path}");
                    }
                }
                Event::Finished(result) => {
                    let result = current_result(self.cache_health.gate.as_ref(), result);
                    if let Some(gate) = &self.cache_health.gate {
                        self.cache_health.progress = gate.progress();
                    }
                    self.cache_health.rx = None;
                    self.cache_health.gate = None;
                    if self.cache_health.roots != roots {
                        continue;
                    }
                    match result {
                        Ok(records) => {
                            let count = records.len();
                            let bad = records
                                .iter()
                                .filter(|r| r.health == Health::Redownload)
                                .count();
                            let healthy = records
                                .iter()
                                .filter(|r| r.health == Health::Healthy)
                                .count();
                            let metadata = records
                                .iter()
                                .filter(|r| r.health == Health::MetadataOnly)
                                .count();
                            let unknown = count - bad - healthy - metadata;
                            if matches!(self.cache_health.selection, Some(Selection::All)) {
                                self.cache_health.report.records.clear();
                                self.cache_health.report.finished_all_at = epic::now();
                                self.cache_health.report.roots = roots.clone();
                            }
                            for record in records {
                                self.cache_health
                                    .report
                                    .records
                                    .insert(health::key(&record.path), record);
                            }
                            health::save(&self.cache_health.report);
                            self.rebuild_cache_health_links();
                            self.cache_health.status = if count == 0 {
                                "No matching cached downloads found.".into()
                            } else {
                                format!(
                                    "Cache checked: {healthy} healthy; {bad} need redownload; {unknown} not verified; {metadata} metadata only."
                                )
                            };
                        }
                        Err(error) => {
                            if !error.contains("stopped") {
                                if self.cache_health.invalidate_failed_check(&error) {
                                    health::save(&self.cache_health.report);
                                }
                            }
                            self.cache_health.status = error;
                        }
                    }
                    if !self.cache_health.automatic
                        && !self.cache_health.retry_after_work
                        && self.cache_health.pending_manual.is_none()
                    {
                        self.notify(self.cache_health.status.clone());
                    }
                }
            }
        }
        if let Some(selection) = self.cache_health.take_pending_manual(transfer_busy) {
            self.start_cache_verification(selection, false, ctx);
        } else if self.cache_health.retry_after_work
            && !transfer_busy
            && self.cache_health.rx.is_none()
        {
            self.cache_health.retry_after_work = false;
            let selection = self
                .cache_health
                .selection
                .clone()
                .unwrap_or(Selection::All);
            self.start_cache_verification(selection, self.cache_health.automatic, ctx);
        }
        if !self.cache_health.startup_checked && !self.cache_health.busy() {
            self.cache_health.startup_checked = true;
            if self.prefs.verify_cache_on_startup
                && (self.cache_health.report.roots != roots
                    || health::due(self.cache_health.report.finished_all_at, epic::now()))
            {
                self.start_cache_verification(Selection::All, true, ctx);
            }
        }
        if self.cache_health.pending_manual.is_some() {
            self.cache_health.status = if transfer_busy {
                "Selected verification queued; waiting for the current operation to finish."
            } else {
                "Selected verification queued; waiting for the current check to stop."
            }
            .into();
        }
        // No timer, polling thread or animation remains after the pass completes.
        if self.cache_health.busy() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
    pub(super) fn cache_health_controls(&mut self, ui: &mut Ui) {
        if downloads::toggle(
            ui,
            &self.theme.clone(),
            &mut self.prefs.verify_cache_on_startup,
            "Verify Vault Cache on startup, at most once a day",
            "Checks cached files without a speed limit while you use the launcher. Waits during downloads and project operations. Does not change cached files or download asset or plugin payloads.",
        ) {
            self.persist();
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(buttons::secondary_button("Verification details"))
                .clicked()
            {
                self.cache_health.details_open = true;
            }
            if ui
                .add(buttons::secondary_button("Verify cache now"))
                .clicked()
            {
                self.start_cache_verification(Selection::All, false, ui.ctx());
            }
            if self.cache_health.busy()
                && ui
                    .add(buttons::secondary_button("Stop verification"))
                    .clicked()
            {
                self.cache_health.cancel();
            }
        });
        if !self.cache_health.status.is_empty() {
            ui.add(
                Label::new(
                    RichText::new(self.cache_health.detail())
                        .size(14.)
                        .color(color(self.theme.muted)),
                )
                .wrap(),
            );
        }
        ui.label("Green: cached files passed verification. Orange: files are missing or damaged and need downloading again. Manual Verify can retrieve original file hashes for older plugin downloads from your Epic account. Metadata-only folders contain no package files to check.");
    }
    pub(super) fn cache_verification_visible(&self) -> bool {
        self.cache_health.busy()
            || (self.cache_health.selection.is_some()
                && !self.cache_health.status.is_empty()
                && matches!(self.page, Page::Library | Page::VaultCache | Page::Settings))
    }
    pub(super) fn cache_verification_footer(&mut self, ui: &mut Ui, c: &Canvas) {
        let rect = c.rect([271., c.height - 50., c.width - 297., 46.]);
        let theme = self.theme.clone();
        let stop = self.cache_health.busy();
        let actions_width = if stop { 158. } else { 82. };
        let content = Rect::from_min_max(
            rect.min + vec2(8., 4.),
            pos2(rect.right() - actions_width - 12., rect.bottom() - 4.),
        );
        ui.scope_builder(UiBuilder::new().max_rect(content), |ui| {
            ui.set_width(content.width());
            ui.spacing_mut().item_spacing.y = 1.;
            ui.add(
                Label::new(
                    RichText::new(self.cache_health.heading())
                        .size(13.)
                        .color(color(theme.text)),
                )
                .truncate(),
            )
            .on_hover_text(self.cache_health.heading());
            ui.add(
                Label::new(
                    RichText::new(self.cache_health.detail())
                        .size(12.)
                        .color(color(theme.muted)),
                )
                .truncate(),
            )
            .on_hover_text(self.cache_health.detail());
            if stop && self.cache_health.progress.files_total > 0 {
                ui.add(
                    ProgressBar::new(self.cache_health.fraction())
                        .desired_width(content.width())
                        .desired_height(3.),
                );
            }
        });
        let actions = Rect::from_min_size(
            pos2(
                rect.right() - actions_width - 6.,
                rect.center().y - COMPACT_BUTTON_HEIGHT / 2.,
            ),
            vec2(actions_width, COMPACT_BUTTON_HEIGHT),
        );
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(actions)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                if stop && ui.add(buttons::secondary_button("Stop")).clicked() {
                    self.cache_health.cancel();
                }
                if ui.add(buttons::secondary_button("Details")).clicked() {
                    self.cache_health.details_open = true;
                }
            },
        );
    }
    pub(super) fn cache_verification_dialog(&mut self, ctx: &Context) {
        if !self.cache_health.details_open {
            return;
        }
        let mut open = true;
        let theme = self.theme.clone();
        dialog::show(
            ctx,
            &theme,
            "cache-verification",
            "Vault Cache verification",
            "Read-only checks of cached downloads. Installed projects are not checked or changed.",
            &mut open,
            |ui| {
                buttons::compact_style(ui);
                ui.label(
                    RichText::new(self.cache_health.heading())
                        .strong()
                        .color(color(theme.text)),
                );
                ui.add(Label::new(self.cache_health.detail()).wrap());
                ui.separator();
                ui.label(if self.cache_health.automatic {
                    "Check: Automatic daily startup check"
                } else {
                    "Check: Manual verification"
                });
                if let Some(selection) = &self.cache_health.selection {
                    ui.add(Label::new(format!("Requested: {}", selection_title(selection))).wrap());
                }
                if let Some(selection) = &self.cache_health.pending_manual {
                    ui.add(
                        Label::new(format!("Queued next: {}", selection_title(selection))).wrap(),
                    );
                }
                let p = &self.cache_health.progress;
                if !p.entry.as_os_str().is_empty() {
                    ui.add(Label::new(format!("Cache folder: {}", p.entry.display())).wrap());
                }
                if !p.file.as_os_str().is_empty() {
                    ui.add(Label::new(format!("Current file: {}", p.file.display())).wrap());
                }
                ui.label(format!(
                    "{} / {} folders inspected · {} / {} files checked in this folder",
                    p.entries_completed, p.entries_total, p.files_completed, p.files_total
                ));
                if p.files_total > 0 {
                    ui.add(
                        ProgressBar::new(self.cache_health.fraction())
                            .desired_width(ui.available_width())
                            .desired_height(6.)
                            .fill(color(theme.accent)),
                    );
                }
                if p.file_total > 0 {
                    ui.label(format!(
                        "Current file: {:.1} / {:.1} MB read",
                        p.file_bytes as f64 / 1_000_000.,
                        p.file_total as f64 / 1_000_000.
                    ));
                }
                ui.label(format!(
                    "Total read this check: {:.1} MB · No speed limit",
                    p.bytes_read as f64 / 1_000_000.
                ));
                if self.cache_health.busy()
                    && ui
                        .add(buttons::secondary_button("Stop verification"))
                        .clicked()
                {
                    self.cache_health.cancel();
                }
            },
        );
        self.cache_health.details_open = open;
    }
    pub(super) fn cache_icon_tint(&self, item: &engine_workspace::fab::Item) -> [u8; 3] {
        let records: Vec<_> = self
            .cache_health
            .for_item(item)
            .into_iter()
            .filter(|r| r.health != Health::MetadataOnly)
            .collect();
        if records.iter().any(|r| r.health == Health::Redownload) {
            self.theme.warning
        } else if !records.is_empty() && records.iter().all(|r| r.health == Health::Healthy) {
            self.theme.success
        } else {
            self.theme.text
        }
    }
    pub(super) fn cached_item_tooltip(&self, item: &engine_workspace::fab::Item) -> String {
        let records = self.cache_health.for_item(item);
        if records.is_empty() {
            "Cached files have not been verified. Right-click the type badge or thumbnail and choose Verify cached files. Installed project files are not checked.".into()
        } else {
            records
                .into_iter()
                .map(tooltip)
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    }
    pub(super) fn cache_entry_health(&self, ui: &mut Ui, path: &Path) {
        let record = self.cache_health.for_path(path);
        let (text, col, tip) = record
            .map(|r| (label(r.health), tint(&self.theme, r.health), tooltip(r)))
            .unwrap_or((
                "Not verified",
                self.theme.muted,
                "Use Verify cached files in the item menu.".into(),
            ));
        ui.label(RichText::new(text).size(14.).color(color(col)))
            .on_hover_text(tip);
    }
    pub(super) fn preview_cache_health(&mut self) {
        let status_preview = std::env::args().find_map(|a| {
            a.strip_prefix("--cache-health-status-preview=")
                .map(str::to_owned)
        });
        if std::env::args().any(|a| a == "--cache-health-busy-preview")
            || status_preview.as_ref().is_some_and(|s| s != "complete")
        {
            // Render only: exercise active menu actions during a background pass,
            // without reading or changing the user's cache.
            let (_tx, rx) = mpsc::channel();
            self.cache_health.rx = Some(rx);
            self.cache_health.gate = Some(Gate::default());
            self.cache_health.automatic = true;
        }
        if let Some(state) = status_preview {
            self.cache_health.selection = Some(Selection::All);
            self.cache_health.progress = health::Progress {
                entry: "C:/ProgramData/Epic/EpicGamesLauncher/VaultCache/EngineWorkspace/Chunks".into(),
                file: "C:/ProgramData/Epic/EpicGamesLauncher/VaultCache/EngineWorkspace/Chunks/7C5BFA43D9E0784B1E986572AC09BE6AD14C47B3".into(),
                phase: "Checking file hashes".into(), entries_completed: 2, entries_total: 5,
                files_completed: 128, files_total: 400, file_bytes: 8_000_000,
                file_total: 16_000_000, bytes_read: 512_000_000,
            };
            self.cache_health.paused = state == "paused";
            self.cache_health.pause_reason =
                "Waiting for the Library download or project operation to finish.".into();
            if state == "queued" {
                self.cache_health
                    .prioritize(Selection::Entry("C:/VaultCache/PCGEx".into()));
            }
            if state == "complete" {
                self.cache_health.status =
                    "Cache checked: 3 healthy; 1 need redownload; 1 not verified.".into();
            }
            self.cache_health.details_open =
                std::env::args().any(|a| a == "--cache-health-details-preview");
        }
        self.cache_health.roots = self.prefs.vault_cache_roots();
        self.cache_health.report.records.clear();
        let root = std::path::PathBuf::from(&self.cache_health.roots[0]);
        let fixtures: Vec<_> = if self.page == Page::VaultCache {
            self.vault
                .preview_entries()
                .into_iter()
                .map(|e| (e.path, e.title, e.identities))
                .collect()
        } else {
            self.library
                .cache_items()
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    (
                        root.join(format!("Cache item {i}")),
                        item.title.clone(),
                        vec![item.asset_id.clone()],
                    )
                })
                .collect()
        };
        for (index, (path, title, identities)) in fixtures.into_iter().enumerate() {
            let health = match index % 3 {
                0 => Health::Healthy,
                1 => Health::Redownload,
                _ => {
                    if std::env::args().any(|a| a == "--cache-health-metadata-preview") {
                        Health::MetadataOnly
                    } else {
                        Health::Unknown
                    }
                }
            };
            let record = Record {
                root: root.clone(),
                path: path.clone(),
                title,
                identities,
                health,
                detail: match health {
                    Health::Healthy => "All cached files passed hash verification.",
                    Health::Redownload => {
                        "One cached file is missing or damaged. Download the package again."
                    }
                    Health::Unknown => "No saved file hashes are available.",
                    Health::MetadataOnly => "This folder contains download metadata only; there are no package files to check.",
                }
                .into(),
                checked_at: epic::now(),
                files: 25,
            };
            self.cache_health
                .report
                .records
                .insert(health::key(&path), record);
        }
        self.rebuild_cache_health_links();
    }
    pub(super) fn rebuild_cache_health_links(&mut self) {
        self.cache_health.relink(self.library.cache_items());
    }
}
