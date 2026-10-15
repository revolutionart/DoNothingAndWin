<p align="center"><img src="assets/brand.png" alt="DoNothing&Win Launcher" width="420"></p>

# DoNothing&Win Launcher

A Windows launcher for **Unreal Engine developers**. Engines, projects, Fab assets, plugins, and downloads. No game library, game store, friends list, chat, or other social features.

## Why this exists

A fun "fine, I'll vibe-code my own" response to the new vibe coded launcher experience. Written with Codex for Unreal Engine developers: engines, projects, Fab assets, plugins, and downloads. No games or social features. Independent of Epic Games.

This is an early community release, not a claim that AI-generated software is automatically reliable. We all know the true state. Bug reports, testing, and contributions are welcome.

## Screenshots

Click any screenshot to view it at full size.

| Workspace | Light theme |
| --- | --- |
| ![Workspace](docs/screenshots/DNW_Workspace.png) | ![Light theme](docs/screenshots/DNW_LightTheme.png) |

| Projects and categories | Fab library |
| --- | --- |
| ![Projects and categories](docs/screenshots/DNW_ProjectCatgories.png) | ![Fab library](docs/screenshots/DNW_FabLibrary.png) |

| Engine plugins | Engine versions |
| --- | --- |
| ![Engine plugins](docs/screenshots/DNW_Plugins.png) | ![Engine versions](docs/screenshots/DNW_EngineVersions.png) |

| Engine installation | Installation progress |
| --- | --- |
| ![Engine installation](docs/screenshots/DNW_EngineInstall.png) | ![Installation progress](docs/screenshots/DNW_EngineInstaller_Progression.png) |

| Downloads | Settings |
| --- | --- |
| ![Downloads](docs/screenshots/DNW_Downloads.png) | ![Settings](docs/screenshots/DNW_Settings.png) |

## Download and install

Download the **[1.0.0 Windows installer](https://github.com/revolutionart/DoNothingAndWin/releases/download/v1.0.0/DoNothingAndWin-Setup-1.0.0.exe)**, or get the portable ZIP and SHA-256 checksums from the **[1.0.0 release](https://github.com/revolutionart/DoNothingAndWin/releases/tag/v1.0.0)**.

- Windows 10/11, 64-bit. Embedded Epic sign-in needs the [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/).
- Setup installs for your Windows user; administrator access is not required for the launcher itself. Writing to protected engine folders may require appropriate folder permissions.
- **Launch after setup** is checked by default.
- **Start with Windows** is optional. When selected, **start minimized** is enabled by default and can be unchecked during setup.
- Minimize keeps the launcher in the taskbar; Close (X) hides it in the system tray. Right-click the tray icon and choose Exit to quit. Downloads settings include an option to start minimized with Windows.
- Setup and uninstall do not delete your Unreal projects or engine installations. Uninstall removes launcher files and its startup entry; local launcher settings are retained.
- Setup refuses to replace a running launcher. Finish your transfers and exit before installing an update. It never force-closes a download.

The first release is unsigned; Windows may display an unknown-publisher/SmartScreen prompt. Download only from this repository's releases and compare the provided SHA-256 checksums if needed.

## What it does

### Projects and Workspace

- Discover projects in configured folders, import an existing `.uproject`, or create a blank project.
- Double-click a project to open it with its associated engine. Missing-engine indicators help identify projects that need another engine installed.
- Switch between grid and list views, search with a clear button, filter by engine version, and sort by name or recently opened.
- Browse **All projects**, **Pinned**, and your own category tabs.

Right-click a project, or open its **three-dot menu**, for these actions:

| Option | What it does |
| --- | --- |
| **Categories** | Assign a category, remove the assignment, or open the category manager. |
| **Hide project** | Keep a project out of the launcher’s project lists without deleting or moving its files. Useful for screenshots or keeping your workspace focused. |
| **Create desktop shortcut** | Create a shortcut to open the project. |
| **Clone project** | Make a separate copy of a project. |
| **Pin project / Unpin** | Add or remove the project from the Pinned tab. |
| **Show in Folder** | Open the project’s folder in Windows Explorer. |

**Manage categories** sits next to Refresh. Deleting a category removes its assignments; its projects remain under All projects unless hidden. Hidden projects stay hidden across restarts. Restore one project or all of them in **Settings → Hidden projects**.

### Engine versions

- Discover installed engines, choose the current engine, and launch Unreal Editor.
- Install engine builds with a choice of destination and components, download/storage estimates, and Epic installation registration.
- Use an engine’s three-dot menu to change components through **Options**, **Set Current**, **Create Shortcut**, **Show in Folder**, **Manage Plugins**, **Verify**, or **Remove**.
- Check for available engine releases while the launcher is open. Installations and updates are started manually.

### Fab library and plugins

- Browse your owned Fab content under **All**, **Assets**, **Projects**, and **Plugins**, with search, sorting, and engine-version filtering.
- Add assets to an existing project, create projects from supported library templates, and install compatible plugins into a selected engine.
- Enable **Show all projects** when choosing an asset destination. Projects too old for every available asset version are greyed out, with a reason on hover.
- Manage plugins separately for each engine: search installed plugins, see installed and available versions, check for updates, update, or remove them.
- Sidebar indicators highlight available plugin updates beside Library and Engine versions.

### Downloads and activity

- Follow installation progress, transferred bytes, and the active-download indicator beside Downloads in the sidebar.
- Stop an engine transfer and resume it using the same destination to reuse verified files and cached chunks.
- **Dismiss** clears a stopped installation’s resume entry without deleting retained files. Removing its pending engine card also clears the entry.
- Search installation, removal, and verification history; inspect operation details, mark entries as seen, or clear history.
- Set the default engine installation folder, a shared download-speed limit, and installation-completion notifications.

### Everyday use

- Dark and daylight themes, remembered window size, and cached Workspace checks refreshed in the background.
- Optional Windows startup and minimized startup. Minimize keeps the app in the taskbar; Close (X) hides it in the tray, where **Exit** quits it.
- A single launcher instance, with build handoff allowed only when idle so active transfers are preserved.

This does not include Unreal Engine, paid assets, plugins, or games. You use your own Epic account and entitlements. Engine and asset license terms remain their owners' terms. Older assets may be offered for explicit import into a newer project; compatibility is not guaranteed. Assets requiring a newer engine are blocked from older projects.

## Your files and account

The launcher operates on the engines and projects you choose. Installing or uninstalling **the launcher** does not remove those projects. Explicit in-app engine/plugin removal actions do remove the selected engine/plugin.

Local settings, cached library data, activity, and the encrypted Epic session live under `%LOCALAPPDATA%\EngineWorkspace` (the historical internal folder name). On Windows, saved session credentials use DPAPI.

## Build from source

Install stable Rust with the MSVC toolchain and Visual Studio C++ Build Tools / Windows SDK, then:

```powershell
cargo build --release --locked --bins
cargo test --locked --lib --bins --tests
.\target\release\engine-workspace.exe
```

Live account/CDN tests are ignored by default. One offline streaming test writes and verifies a temporary file slightly over 1 GiB, so allow space on your temporary drive.

To build the installer, install [Inno Setup 6](https://jrsoftware.org/isinfo.php), then run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package.ps1
```

Python 3 is used to gather dependency license notices. Release artifacts are written to `dist/`. See [CONTRIBUTING.md](CONTRIBUTING.md) for development and release details.

## License and independence

[MIT](LICENSE). Third-party dependencies retain their own licenses; packaged builds include their license notices. Epic Games, Unreal Engine, and Fab names belong to their respective owners. This is an independent community project, not an official Epic Games product.
