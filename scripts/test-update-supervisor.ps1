param([string]$Launcher = 'target/release/engine-workspace.exe')
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$launcherPath = (Resolve-Path -LiteralPath $Launcher).Path
$fixtureRoot = Join-Path (Get-Location) ('review/update-supervisor-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
# Both installer and reopened launcher are harmless fixtures. No real installation,
# account, cache or instance lock is accessed by this test.
$source = @'
using System;
using System.IO;
using System.Threading;
public class Probe {
    public static int Main() {
        string exe = System.Reflection.Assembly.GetExecutingAssembly().Location;
        string root = Directory.GetParent(Path.GetDirectoryName(exe)).FullName;
        if (Path.GetFileName(exe) == "installer.exe") {
            File.AppendAllText(Path.Combine(root, "setup-count.txt"), "setup\n");
            int mode = Int32.Parse(File.ReadAllText(Path.Combine(root, "mode.txt")));
            if (mode == 9) File.Delete(Path.Combine(root, "installed", "DoNothingAndWinLauncher.exe"));
            return mode;
        }
        string lockFile = Path.Combine(root, "local", "EngineWorkspace", "launcher-instance.lock");
        using (var stream = new FileStream(lockFile, FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.ReadWrite)) {
            stream.Lock(0, Int64.MaxValue);
            File.AppendAllText(Path.Combine(root, "reopen-count.txt"), "reopen\n");
            Thread.Sleep(2000);
            stream.Unlock(0, Int64.MaxValue);
        }
        return 0;
    }
}
'@
$sourcePath = Join-Path $fixtureRoot 'Probe.cs'
Set-Content -LiteralPath $sourcePath -Value $source -Encoding UTF8
$compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
$probeExe = Join-Path $fixtureRoot 'Probe.exe'
& $compiler /nologo /target:winexe ('/out:' + $probeExe) $sourcePath
if ($LASTEXITCODE -ne 0) { throw 'Fixture compilation failed' }
foreach ($mode in @(0, 1, 2, 5, 7, 9)) {
    $caseRoot = Join-Path $fixtureRoot "case-$mode"
    $stage = Join-Path $caseRoot 'stage'
    $installed = Join-Path $caseRoot 'installed'
    $local = Join-Path $caseRoot 'local'
    New-Item -ItemType Directory -Path $stage,$installed,(Join-Path $local 'EngineWorkspace') | Out-Null
    Copy-Item -LiteralPath $launcherPath -Destination (Join-Path $stage 'update-supervisor.exe')
    Copy-Item -LiteralPath $probeExe -Destination (Join-Path $stage 'installer.exe')
    Copy-Item -LiteralPath $probeExe -Destination (Join-Path $installed 'DoNothingAndWinLauncher.exe')
    Set-Content -LiteralPath (Join-Path $installed 'unins000.exe') -Value 'fixture marker'
    Set-Content -LiteralPath (Join-Path $caseRoot 'mode.txt') -Value $mode
    $request = @{ directory = $installed; update = @{ version='9.9.9'; release_url='https://github.com/revolutionart/DoNothingAndWin/releases/tag/v9.9.9'; download_url='https://github.com/revolutionart/DoNothingAndWin/releases/download/v9.9.9/DoNothingAndWin-Setup-9.9.9.exe'; size=(Get-Item $probeExe).Length; sha256=(Get-FileHash $probeExe -Algorithm SHA256).Hash.ToLower() } }
    [IO.File]::WriteAllText((Join-Path $stage 'handoff.json'), ($request | ConvertTo-Json -Depth 4), [Text.UTF8Encoding]::new($false))
    $lockPath = Join-Path $local 'EngineWorkspace/launcher-instance.lock'
    $parentLock = [IO.File]::Open($lockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::ReadWrite)
    $parentLock.Lock(0, [long]::MaxValue)
    $start = [Diagnostics.ProcessStartInfo]::new((Join-Path $stage 'update-supervisor.exe'), '--supervise-update')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.EnvironmentVariables['LOCALAPPDATA'] = $local
    $helper = [Diagnostics.Process]::Start($start)
    try {
        $deadline = [DateTime]::UtcNow.AddSeconds(20)
        while (-not (Test-Path (Join-Path $stage 'ready'))) {
            if ($helper.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Helper failed before readiness' }
            Start-Sleep -Milliseconds 100
        }
        $writeBlocked = $false
        try {
            $writeProbe = [IO.File]::Open((Join-Path $stage 'installer.exe'), [IO.FileMode]::Open, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite)
            $writeProbe.Dispose()
        } catch [IO.IOException] { $writeBlocked = $true }
        if (-not $writeBlocked) { throw 'Verified installer remained writable before execution' }
        Set-Content -LiteralPath (Join-Path $stage 'proceed') -Value 'proceed'
        Start-Sleep -Milliseconds 300
        if (Test-Path (Join-Path $caseRoot 'setup-count.txt')) { throw 'Setup ran before parent released its lock' }
        $parentLock.Dispose()
        $deadline = [DateTime]::UtcNow.AddSeconds(25)
        $resultPath = if ($mode -eq 0) { Join-Path $caseRoot 'reopen-count.txt' } else { Join-Path $stage 'recovery.txt' }
        while (-not (Test-Path $resultPath)) {
            if ([DateTime]::UtcNow -gt $deadline) { throw "No outcome for mode $mode" }
            Start-Sleep -Milliseconds 100
        }
        if (@(Get-Content (Join-Path $caseRoot 'setup-count.txt')).Count -ne 1) { throw 'Setup was retried' }
        if ($mode -eq 9) {
            if (Test-Path (Join-Path $caseRoot 'reopen-count.txt')) { throw 'Tried to reopen a missing launcher' }
            if ((Get-Content $resultPath -Raw) -notmatch 'official release installer') { throw 'Missing manual recovery advice' }
        } else {
            if (@(Get-Content (Join-Path $caseRoot 'reopen-count.txt')).Count -ne 1) { throw 'Launcher was not reopened exactly once' }
            if ($mode -ne 0 -and (Get-Content $resultPath -Raw) -notmatch 'retry it manually') { throw 'Missing retry advice' }
        }
        Write-Output "PASS: supervisor mode $mode; waited for parent, one setup attempt, correct restart/recovery."
    } finally {
        $parentLock.Dispose()
        # Close only this test's helper, including its fixture recovery dialog.
        if (-not $helper.HasExited) { $helper.Kill(); $helper.WaitForExit() }
    }
}
