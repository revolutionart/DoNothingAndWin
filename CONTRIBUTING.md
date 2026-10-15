# Contributing

Small, focused issues and pull requests are welcome. Include the launcher version, Windows version, expected behavior, and reproduction steps. Redact tokens, account identifiers, and private paths from screenshots and logs.

Run `cargo fmt --check` and `cargo test --locked --lib --bins --tests` before submitting code. Never run ignored live repair/install tests against important data. They require explicit environment configuration.

Keep disk and network work off the UI thread. Preserve download-safe single-instance behavior, streaming writes, path traversal checks, and installation rollback. Never commit local account state or downloaded marketplace content.

## Release

1. Update `Cargo.toml` and `Cargo.lock`, then test and build with the lockfile.
2. Run `scripts/package.ps1` with Inno Setup 6 installed. The installer version comes from Cargo.toml.
3. Test install, launch, startup/minimized choices, upgrade refusal while running, and uninstall in an isolated Windows environment. Verify projects outside the application folder are untouched.
4. Tag the reviewed commit `vVERSION`. The tag workflow builds installer/portable ZIP/checksums as Actions artifacts for review; publication is manual.
5. Attach the installer, portable ZIP, and SHA256SUMS.txt to a GitHub release.

The installer is per-user and never recursively deletes user content. Keep startup registration scoped to the current user. The application name is DoNothing&Win Launcher; `engine-workspace` and the local data folder name remain internal compatibility names.
