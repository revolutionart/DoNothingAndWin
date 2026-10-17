# Contributing

Small, focused issues and pull requests are welcome. Include the launcher version, Windows version, expected behavior, and reproduction steps. Redact tokens, account identifiers, and private paths from screenshots and logs.

Run `cargo fmt --check` and `cargo test --locked --lib --bins --tests` before submitting code. Never run ignored live repair/install tests against important data. They require explicit environment configuration.

Also run `cargo test --locked --features portable --bin engine-workspace` for the portable variant. On a Windows desktop, the isolated native-window regression test can be run with `cargo test --release --locked --bin engine-workspace background_window_processes_workers_and_exits_cleanly -- --ignored`; repeat with `--features portable`. It checks background wakeups and normal shutdown without loading accounts or project data.

Keep disk and network work off the UI thread. Preserve download-safe single-instance behavior, streaming writes, path traversal checks, and installation rollback. Never commit local account state or downloaded marketplace content.

For launcher updater changes, build the installed edition with `cargo build --release --locked --bin engine-workspace`, then run `scripts/test-update-supervisor.ps1`. It uses stand-in executables and an isolated data directory to check shutdown waiting, verified-file protection, successful restart, cancellation, setup failure and a missing launcher. Recovery dialogs belong to the fixture and are closed by the test. `scripts/test-launcher-update.ps1 -Iscc <path-to-ISCC.exe>` checks the production Inno Setup logic with an isolated AppId and payload, including supervisor-owned restart. `scripts/test-postinstall-launch.ps1 -TargetDir target` checks the Windows desktop handoff against a temporary cache junction. These scripts do not update the real launcher or its engines.

## Release

1. Update `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, and the README download links, then test and build with the lockfile.
2. Run `scripts/package.ps1` with Inno Setup 6 installed. The installer version comes from Cargo.toml. The script builds separate installed and portable executables; the portable build uses `--features portable` and never creates a tray icon.
3. Test install, launch, startup/minimized choices, upgrade refusal while running, and uninstall in an isolated Windows environment. Verify projects outside the application folder are untouched.
4. Tag the reviewed commit `vVERSION`. The tag workflow builds installer/portable ZIP/checksums as Actions artifacts for review; publication is manual.
5. Attach the installer, portable ZIP, and SHA256SUMS.txt to a GitHub release.

The installer is per-user and never recursively deletes user content. Keep startup registration scoped to the current user. The application name is DoNothing&Win Launcher; `engine-workspace` and the local data folder name remain internal compatibility names.
