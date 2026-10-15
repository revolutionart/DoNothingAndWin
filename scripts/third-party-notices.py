"""Collect Windows dependency notices from Cargo's resolved source packages."""
import json, subprocess
from pathlib import Path
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", "x86_64-pc-windows-msvc"]))
reachable = {node["id"] for node in metadata["resolve"]["nodes"]}
parts = ["Third-party dependency notices\n\nDependencies retain their own licenses. The MIT license for DoNothing&Win does not replace these terms.\n"]
for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
    if package["id"] not in reachable or package["source"] is None:
        continue
    root = Path(package["manifest_path"]).parent
    parts.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nLicense: {package.get('license') or 'See license file'}\nSource: {package.get('repository') or package['source']}\n")
    licenses = sorted({p for p in root.rglob('*') if p.is_file() and (p.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE')) or str(p) == str(root / (package.get('license_file') or '__none__')))})
    for path in licenses:
        if path.stat().st_size < 500_000:
            parts.append(f"\n--- {path.relative_to(root)} ---\n" + path.read_text(encoding='utf-8', errors='replace'))
Path('dist/app/THIRD_PARTY_NOTICES.txt').write_text('\n'.join(parts), encoding='utf-8')
