# Changelog

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
