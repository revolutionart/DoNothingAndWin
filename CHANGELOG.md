# Changelog

## 1.1.0 — 2026-10-15

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

## 1.0.0 — 2026-10-15

Initial community release: project discovery and organization; engine installation, verification and removal; owned Fab library browsing; engine plugin management; download progress and activity history; dark/light themes; optional Windows startup.
