param([string]$Iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", [switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
if (-not $SkipBuild) {
    cargo build --release --locked --bins
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
}
if (-not (Test-Path -LiteralPath $Iscc)) { throw 'Inno Setup not found. Pass -Iscc C:\path\ISCC.exe' }
New-Item -ItemType Directory -Force dist/app | Out-Null
Copy-Item target/release/engine-workspace.exe dist/app/DoNothingAndWinLauncher.exe
Copy-Item LICENSE,README.md dist/app
python scripts/third-party-notices.py
if ($LASTEXITCODE -ne 0) { throw 'Dependency notice generation failed' }
& $Iscc "/DAppVersion=$version" installer/launcher.iss
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed' }
Compress-Archive -Path dist/app/* -DestinationPath "dist/DoNothingAndWin-Portable-$version.zip" -Force
$files = @("dist/DoNothingAndWin-Setup-$version.exe", "dist/DoNothingAndWin-Portable-$version.zip")
$files | ForEach-Object { $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $_; "$($hash.Hash.ToLower())  $([IO.Path]::GetFileName($_))" } | Set-Content -Encoding ascii dist/SHA256SUMS.txt
