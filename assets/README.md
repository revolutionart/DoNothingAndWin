# App icon

`brand_icon.png` is the original 400 × 400 RGBA artwork supplied by the maintainer.
The window/taskbar use this PNG directly. `brand_icon.ico` supplies 16, 20, 24,
32, 40, 48, 64, 96, 128 and 256 pixel frames for Windows shell scaling. The
executable, tray and installer share that ICO; desktop/Start menu shortcuts and
the uninstall entry use the executable's embedded icon.

After replacing the PNG, regenerate the ICO with
`python scripts/generate-app-icon.py` (requires Pillow). Verify all frames match
the artwork with `python scripts/generate-app-icon.py --check`.
On Windows, add `--exe path/to/DoNothingAndWinLauncher.exe` to verify every
embedded frame in a compiled executable. This reads its resources without running it.

Keep the source's transparency and original resolution. The large shell frame
is downsampled from the original artwork rather than enlarged from a small icon.
