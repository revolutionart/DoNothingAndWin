# Launcher control consistency

Use the `ui-control-consistency` skill when it is available. This guide contains the launcher-specific contract and remains applicable when the personal skill is unavailable.

## Shared controls

- `src/ui/buttons.rs` owns the compact button style, fonts and glyph centering. Read its constants and helpers before changing a control.
- `src/ui.rs` owns `search_field`, `Canvas::control_text`, `Canvas::styled_button`, icon geometry and canvas scaling. Use `search_field` for toolbar searches, including Vault Cache and Library. A plain `TextEdit` inherits body metrics and is not a toolbar search replacement.
- `src/ui/scrollbar.rs` owns `AppComboBox` and themed scrolling. Use it for launcher dropdowns, with `buttons::selectable_value` for options so the open list retains the same glyph alignment as the selected label.
- `src/ui/loading.rs` owns every circular loader through `loading::spinner(theme)`. It fixes the small size, stroke, animation and theme contrast. Callers only choose placement, including inside larger button/overlay slots. Every dark-theme loader is white; every light-theme loader is dark. Use this component for inline, sidebar, button, footer and dialog loaders. Run `scripts/check-ui-controls.ps1`; CI rejects standalone spinner construction elsewhere.
- `src/theme.rs` and `themes/` own the other theme colors.

## Geometry contract

Controls in a toolbar share the compact control height from source, one row center and consistent padding. Canvas positions are design coordinates; flow widgets use logical UI points. Apply canvas scale once when crossing between them. OS display scaling then maps logical points to physical pixels.

Button labels, selected dropdown text and adjacent icons must align to a stable font-specific baseline. The shared helper normalizes text to the capital-height box of Segoe UI / Semibold. Descenders extend below that baseline; they must not pull a whole word upward. A word's complete glyph mesh center is not its typographic center. Preserve control heights and hit areas while normalizing text, and keep its origin on the font pixel grid so renderer rounding cannot shift different buttons differently. The shared text layout uses an even physical-pixel height; painted button bounds are snapped before placing their contents. Verify this at both toolbar row positions, since nominally centered text can round differently. Align an icon and label as a group; derive positions from the control center and measured text width.

Preserve typing, clearing, focus, tooltips, menus, disabled states and text contrast. Reuse the established hover and warning colors. When fixing a shared control, inspect its callers in page toolbars and dialogs before declaring it done.

## Verification

Run the relevant UI geometry tests (`cargo test --locked --bin engine-workspace`), then build and render the affected pages with the existing layout probe. Discover current probe arguments in `src/ui/layout_probe.rs`; outputs belong under ignored `target/` folders. For controls shared by these pages, cover Workspace, Projects, Library and Vault Cache in dark and light themes, including the smallest supported window. The probe's widths are logical points; record the actual native display scale separately.

Inspect full rows and tight crops at native resolution. Compare the initial capital's visible pixel center, control edges, dropdown arrows and the icon's visible bounds; include labels with and without descenders. Check a search field both empty and filled, plus hover/disabled controls and an open dropdown or action menu. Nominal galley boxes and complete-word mesh bounds can both pass while the text still looks raised. Add a regression test at the actual icon-button call site and verify it catches the previous behavior.
