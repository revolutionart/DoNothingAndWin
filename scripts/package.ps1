param(
    [string]$Iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    [switch]$SkipBuild,
    [string]$InstalledExe,
    [string]$PortableExe
)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
if ($SkipBuild) {
    if (-not $InstalledExe -or -not $PortableExe) {
        throw '-SkipBuild requires separate -InstalledExe and -PortableExe paths, built without and with --features portable respectively.'
    }
} else {
    cargo build --release --locked --bins
    if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    $cargoMetadata = cargo metadata --no-deps --locked --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
    $InstalledExe = Join-Path $cargoMetadata.target_directory 'release/engine-workspace.exe'
}
if (-not (Test-Path -LiteralPath $Iscc)) { throw 'Inno Setup not found. Pass -Iscc C:\path\ISCC.exe' }
New-Item -ItemType Directory -Force dist/app | Out-Null
Copy-Item -LiteralPath $InstalledExe -Destination dist/app/DoNothingAndWinLauncher.exe
Copy-Item LICENSE,README.md dist/app
python scripts/third-party-notices.py
if ($LASTEXITCODE -ne 0) { throw 'Dependency notice generation failed' }
& $Iscc "/DAppVersion=$version" installer/launcher.iss
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed' }
if (-not $SkipBuild) {
    cargo build --release --locked --bin engine-workspace --features portable
    if ($LASTEXITCODE -ne 0) { throw 'Portable build failed' }
    $PortableExe = Join-Path $cargoMetadata.target_directory 'release/engine-workspace.exe'
}
New-Item -ItemType Directory -Force dist/portable | Out-Null
Copy-Item -LiteralPath $PortableExe -Destination dist/portable/DoNothingAndWinLauncher.exe
Copy-Item dist/app/LICENSE,dist/app/README.md,dist/app/THIRD_PARTY_NOTICES.txt dist/portable
Compress-Archive -Path dist/portable/* -DestinationPath "dist/DoNothingAndWin-Portable-$version.zip" -Force
$files = @("dist/DoNothingAndWin-Setup-$version.exe", "dist/DoNothingAndWin-Portable-$version.zip")
$files | ForEach-Object { $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $_; "$($hash.Hash.ToLower())  $([IO.Path]::GetFileName($_))" } | Set-Content -Encoding ascii dist/SHA256SUMS.txt
