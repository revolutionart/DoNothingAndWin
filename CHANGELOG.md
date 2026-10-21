# Changelog

## 1.2.9 - 2026-10-21

### Library

- Fix **Latest added** sorting for recent Fab acquisitions by reading acquisition dates from the Fab library, with legacy Epic entitlement dates as a fallback.
- Preserve cached dates if a refresh fails and show a useful retry message. Items without a recorded acquisition date remain last.

### Project discovery

- Show skipped folder paths and reasons in scan results, including inaccessible folders and folder links. Keep the latest results available in Downloads and through Settings.
- Add **Scan project folders on startup**, off by default. Existing saved choices are preserved; manual refresh and **Rescan folders** remain available.
- Add **Show scan-completion notifications** so scan popups can be disabled independently. Click a scan notification to open its results.

### Interface

- Keep page actions floating at the top right, independent of the page title, so resizing does not push the title or split the action group into separate rows.
- Move **Engine versions** from the sidebar to the top right of Settings, retaining its engine update indicator.

### Known limitations

- Some library items may still lack an acquisition date; **Latest added** reports date coverage.
- Windows 10/11 x64 only. Installer and portable builds remain unsigned.

## 1.2.8 - 2026-10-21

### Library and plugin checks

- Improve matching of owned plugins when their Fab listing link is missing or stale. Verify candidate packages using listing identity or publisher, name and module metadata; keep engine compatibility and ambiguity checks.
- Distinguish an unresolved plugin match from a failed update request, with more useful status details.
- Add **Latest added** sorting using Epic entitlement acquisition dates, newest first. Show date coverage and place items without a recorded date last.
- Show Library card information, hide and folder icons on hover or keyboard focus. Keep folder controls visible while their menu is open.
- Remove the redundant **All categories** dropdown; use the existing category buttons to filter the Library.

### Workspace and preferences

- Show all engine cards in wrapping rows instead of hiding additional engines behind **More engines**.
- Drag engine cards to rearrange them, including between rows. Remember the order across restarts without changing the default engine.
- Sort the project engine-version filter numerically, newest first, with **All versions** at the top.
- Add a space to the main launch label, such as **Launch UE 5.8.4**.
- Add **Close to system tray** in Settings. Turn it off to quit the installed launcher when pressing X; Windows startup behavior remains separately configurable. Portable builds continue to exit on X.

### Known limitations

- Epic does not provide usable acquisition dates for every library item. **Latest added** orders items with available dates first and reports coverage.
- Plugin matching still requires a unique, verified owned package. Unresolved identity or incompatible builds cannot be updated automatically.
- Windows 10/11 x64 only. Installer and portable builds remain unsigned.

## 1.2.7 - 2026-10-20

### Engine updates

- Replace **Launch** with a gold-orange **Update** button when an engine hotfix is available. Start the update directly, keeping the installation path, selected components, plugins and custom files.
- Verify the staged update before replacing engine files, and restore the original installation if activation fails.
- Show verification, registration and cleanup progress instead of leaving Downloads at 100% while work continues.
- Clear the engine update indicator when the release is installed. Show the newest usable launcher-managed hotfix per engine branch so an older copy does not reappear as a separate card; retain distinct custom/source builds.

### Plugin rollback

- Save the current plugin before updating it. Open **Manage plugins → Plugin backups** to restore a saved version, keep it, or delete its backup.
- Keep new backups for seven days by default, with settings for 14 days, 30 days or manual deletion only. **Keep** protects an individual backup from expiry. Expired backups are cleaned up when opening the backup manager or updating plugins for that engine.
- Verify backup contents before restoring to the same engine installation. Save the replaced plugin as another backup so a restore can be undone.
- Restore captured Epic registration when a matching record is available, and report when registration could not be restored.

### Interface

- Dim entire plugin rows while checks are running, including icons, text, status and actions.
- Fix **Manage plugins** hover contrast in the light theme.
- Move **Find projects** to the top toolbar in place of **Scan a folder**, removing the duplicate beside project categories.

### Known limitations

- Plugin backups begin with the next update; previously deleted versions cannot be recovered. Rollback applies to plugins only.
- Older engine copies remain on disk when consolidated into one card.
- Windows 10/11 x64 only. Installer and portable builds remain unsigned.

## 1.2.6 - 2026-10-20

- Show available engine hotfixes on Workspace cards with a warning border and the target version, such as **Update available - UE5.8.4**. Click the status to view engine releases.
- Add the same circled exclamation indicator to **Engine versions** in the sidebar when an installed engine has an update available.
- Use consistent gold-orange warning colors for engine updates, **Manage plugins**, and Library update cards and buttons.
- Rename the selected engine badge to **Default** so it is distinct from update status.
- Remember the window position alongside its size and maximized state across cold starts. Preserve the normal position while minimized or in the tray, and keep the window on-screen if a monitor is disconnected.

## 1.2.5 - 2026-10-19

- Start in the system tray by default when Windows startup is enabled, without a taskbar button. Add a separate **Start in system tray** setting and update the installer option; turn it off to show the window at sign-in.
- Refresh project thumbnails automatically when their screenshot changes. Read and decode images in the background, retry incomplete writes, and preserve custom project artwork priority.

## 1.2.4 - 2026-10-18

### Plugin installation and downloads

- Select multiple engines in **Install to engine**. Resolve the latest eligible compatible plugin package independently for each engine, respecting the Fab library build setting and preserving existing installations.
- Queue additional plugins while another installation is running. Show waiting jobs in Downloads with **Remove** and **Clear waiting list**, and prevent duplicate plugin/engine targets.
- Restore **Stop transfer** for Library installations. Stop the current job and its remaining engines, clean up partial files, and retain completed installations. Other queued jobs continue.
- Record per-engine installation outcomes and recover the waiting queue if an installation worker stops unexpectedly. Keep launcher updates and account changes protected while installation jobs are pending.

### Plugin updates and interface

- Check plugin updates automatically after startup/sign-in without opening Library. Show an orange **Manage plugins** link and circled exclamation on each engine with an available update.
- Enable **Wait for builds listed in the Fab library** by default for new settings and older settings without a saved choice. Preserve explicitly saved choices. This checks library availability, not publisher approval.
- Show engine installation status immediately after the local executable check, without waiting for plugin counting and project history.
- Use gold Download, yellow-green Read and green Write graphs, and restore the blue download icon on engine installation activity.
- Replace the single-engine dropdown with readable multi-engine selection rows.

### Known limitations

- Waiting plugin jobs are kept for the current session only and are not restored after exiting the launcher.
- Windows 10/11 x64 only. Installer and portable builds remain unsigned.

## 1.2.3 - 2026-10-18

### Library organization

- Organize Fab Library items into folders with create, rename, move and delete controls. Organization is saved locally for each Epic account without moving installed files.
- Hide or restore items with the eye icon. **Show hidden** displays only hidden items matching the current filters.
- Move items using the circular folder icon beneath the eye, or right-click a thumbnail for folder actions. Align the folder manager's names, fields and buttons.

### Project discovery

- Add **Find projects** to Workspace, Projects and the empty state. Review suggested folders and project counts before adding them to Settings; one folder can contain multiple projects.
- Search Unreal/Epic history and likely locations first with a bounded **Quick search**. Use **Full scan** for deeper locations or choose a folder directly. Show results during scanning, allow cancellation, and skip asset/source folders inside recognized projects.

### Plugin updates

- Start updates directly from Library cards and list rows. Update the selected plugin for its targeted engine while keeping other engine updates available.
- Show **Updating** with a spinner on the active plugin button and prevent repeated clicks. Follow transfer progress on Downloads.
- Add the optional **Wait for builds listed in the Fab library** setting. It waits for the exact Windows package build to appear in library metadata; this is not a publisher-approval check. The setting is off by default, preserving existing behavior.

### Interface fixes

- Keep dropdowns in the selected launcher theme when Windows uses a different appearance.
- Use larger, squarer checkboxes with blue checked states, lighter hover feedback and readable labels.
- Show tooltips immediately throughout the launcher.

### Known limitations

- Windows 10/11 x64 only. Linux and macOS support are not included.
- Installer and portable builds remain unsigned. Windows may show security prompts.

## 1.2.2 - 2026-10-17

### Launcher updates

- Keep final installer verification and update preparation off the UI thread. Continue processing project clone completion while minimized or hidden.
- Supervise setup from outside the installed launcher. Reopen the installed launcher once after success, cancellation or failure; explain failed updates and leave retrying to the user. If reopening fails, show manual recovery instructions and the setup log location.
- Preserve installer digest checks, active-operation protection, startup choices and the Windows desktop handoff that protects redirected Unreal cache folders.

### Internal architecture

- Give plugin update comparisons one owner shared by the engine dialog, Library and Available Updates.
- Centralize engine operation transitions, cancellation eligibility and retained resume plans; release busy state when a worker disconnects.

### Known limitations

- Installer and portable builds remain unsigned. Windows may show security prompts.
- Recovery improvements apply to updates started from 1.2.2 or later. When updating from 1.2.1, reopen the launcher manually if setup fails or is cancelled.

## 1.2.1 - 2026-10-17

### Launcher updates

- Add **Update launcher** for installed editions: download and verify the release installer, wait for active operations, install into the existing location and restart through the Windows desktop.
- Verify the installer against GitHub's SHA-256 release asset digest and size. Failed or cancelled downloads leave the current installation running.
- Preserve startup and shortcut choices during automatic updates. Portable editions keep downloading the ZIP for manual replacement.
- Existing 1.2.0 users need to install this release manually once to enable the new updater for future releases.

### Installation fix

- Start the launcher through the Windows desktop when **Launch after setup** is selected. This prevents inherited installer restrictions from blocking Unreal Editor's cache folders when they use junctions or symbolic links.
- Keep the installer's security settings enabled. If the desktop handoff is unavailable, show instructions to open the installed launcher from its shortcut.

### Known limitations

- Final installer verification can briefly make the launcher unresponsive on slow storage.
- If an update is waiting for a project clone that finishes while the launcher is minimized or hidden, restore the launcher window so the update can continue.
- If setup fails or is cancelled after the launcher closes, reopen the launcher manually.
- Installer and portable builds remain unsigned. Windows may show security prompts.

## 1.2.0 - 2026-10-17

### Plugin updates

- Check up to four plugins concurrently, reuse HTTP connections and access tokens, and show results as each check finishes. Recent results are cached for five minutes per account and engine, with changed plugin descriptors and installation receipts checked again.
- Clear completed updates immediately across Library, Available Updates and engine dialogs. Preserve updates for other engines and unfinished batch items, and prevent older background checks from restoring cleared notices.
- Show matching compatible versions as **Up to date** and plugins absent from the connected Fab library as **Not available through Fab**. Failed checks and replacement builds remain distinct.
- Improve matching against the complete owned catalog, including publisher namespaces, shortened artifact names, Fab URLs and Epic launcher links. Keep package identity, engine compatibility, ambiguity and downgrade checks.
- Save artifact/build receipts and read matching Epic installation records to detect replacement builds even when a publisher reuses a version label. Support the FabLauncher export integration's separate update feed.
- Refresh the engine plugin dialog with underline tabs, a combined search/action toolbar and highlighted update rows. Keep direct Update and Remove actions, add an installed-folder button, and simplify status summaries and progress text.

### Downloads and engine installations

- Add live **Download**, **Read** and **Write** graphs to engine and plugin installation activity, using measured network and file I/O. Show compressed package size separately from prepared file size and retain the Stop transfer action.
- Reuse verified cached data without redundant reads while retaining size and SHA-1 validation. Corrupt cached data is rejected and fetched again.
- Load engine installation options automatically when opening the install dialog from Workspace or Engine versions.
- Hide installed engine versions from the new-engine picker and prevent duplicate installs. Newer hotfixes remain available.
- Move engine releases closer to the toolbar, add row hover highlighting and move Epic verification details to the footer.

### Library and responsiveness

- Add an **Available Updates** Library tab and category filter, with one count per item and links to the affected engine's plugin updates.
- Load saved Library data off the UI thread, display first-time sync pages progressively and publish listing details incrementally. Reuse the owned catalog across engine checks and thumbnail connections without decoding images twice.
- Save completed metadata batches during long refreshes and back off failed detail requests for five minutes. Optional metadata continues loading in the background without a blocking missing-details notice.
- Hide legacy Marketplace-only entries from Library cards and filters while retaining the official Fab UE Plugin and the complete catalog needed to match installed plugins.
- Synchronize type tabs and category filters, highlight cards with available updates, and show Library counts and sync progress consistently.

### Interface consistency

- Standardize action buttons across the app on 14-pixel text, 32-pixel height and compact padding. Blue buttons share the same lighter hover color; destructive actions retain red hover styling.
- Use consistent grid/list icons, dropdown and context-menu styling, scrollbar sizing and spacing across pages.
- Make engine cards more compact and distribute project and Library grid columns evenly. Fit full thumbnail artwork, round image corners and align project images to the top of their frames.
- Add Recently opened, Name A-Z and Name Z-A sorting to Projects and Workspace.
- Align page headers, dividers and search/filter rows. Add the Settings header divider, darker section backgrounds and more space beside its scrollbar.
- Match Downloads section backgrounds, retain the activity card border on the Downloads page and remove the redundant ACTIVE heading.
- Improve installation loader and checkbox contrast, center and enlarge the plugin dialog close icon, and remove redundant plugin counts and explanatory labels.

### Upgrade notes

- Installer and portable builds are included with SHA-256 checksums. Both remain unsigned.
- Finish transfers and exit the launcher before replacing an installed build. Engine installations, projects and saved preferences are retained.

## 1.1.0 - 2026-10-15

### Launcher updates

- Check for a new stable launcher release in the background at startup and once a day while open, with a manual check in Settings.
- Show an update notice in the sidebar and links to release notes and the installer or portable ZIP appropriate to the running build.
- Checks use public GitHub release metadata without account credentials. Downloads and installation remain manual, so checks do not interrupt transfers or restart the app.
- Version 1.0.0 users need to download this release manually to receive future update notifications.

### Library

- Added engine-version filtering and **Latest update** sorting, now the default. Listing dates load in the background; items without a known date appear last.
- Expanded search to descriptions, tags, and publisher names, with title matches first. Cached listing details improve responsiveness; incomplete metadata is indicated in the UI.
- Library titles open Fab in the default browser.
- Made grid thumbnails the same size as project thumbnails and moved the details icon to the thumbnail's top-right corner, fitting more cards across the window.

### Plugin updates

- Improved matching for Fab plugins with missing marketplace identifiers, using package identity and owned library content. Ambiguous matches remain blocked.
- Added support for Fab listing identifiers that do not map to the legacy Epic catalog identifier.
- Compare readable version names when publishers reuse the numeric version counter. Keep engine compatibility checks and downgrade protection.
- Distinguish unknown update status, missing compatible builds, and unmatched listings from confirmed up-to-date plugins.

### Projects and navigation

- Project category menus list existing categories first, followed by **Add** and, when assigned, **Remove from category**.
- Empty categories can be removed from their tab's context menu or the category manager. Hidden projects still count as category members.
- Improved the category-name field's spacing.
- Read last-opened times from Unreal Editor's local recent-project history, including launches outside this launcher. Unknown times are labeled explicitly.
- Increased mouse-wheel scrolling speed and unified scrollbar styling across pages. Scrollbar thumbs keep a usable minimum size on large lists.

### Background behavior and portable builds

- Fixed an event-loop busy wait while the window was hidden in the tray. Hidden/minimized windows also skip UI rendering and UI-only scans; workers and scheduled release checks continue.
- Made tray **Exit** explicitly request normal shutdown and remove the tray icon, without waiting for a hidden-window repaint.
- Portable builds now minimize to the taskbar and exit when **X** is clicked. They never create a tray icon, including for completion notifications.
- Installed builds retain close-to-tray behavior. Both variants save preferences during normal shutdown.

### Upgrade notes

- Finish active downloads and exit the launcher before installing. Setup refuses to replace a running instance.
- Launcher installation and uninstallation do not remove Unreal projects or installed engines. Explicit in-app removal actions still remove the selected content.
- This release is unsigned. Checksums are supplied with the installer and portable ZIP.
- Plugin update detection has automated regression coverage and live catalog checks; a complete real older-to-newer ACF installation was not available for end-to-end testing.

## 1.0.0 - 2026-10-15

Initial community release: project discovery and organization; engine installation, verification and removal; owned Fab library browsing; engine plugin management; download progress and activity history; dark/light themes; optional Windows startup.
