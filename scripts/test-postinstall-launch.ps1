param([string]$TargetDir = 'target')
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
# Keep every fixture in an isolated review directory. No user cache is changed.
$probeFolder = Join-Path (Get-Location) ('review/desktop-launch-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $probeFolder | Out-Null
$cacheTarget = Join-Path $probeFolder 'cache target'
New-Item -ItemType Directory -Path $cacheTarget | Out-Null
New-Item -ItemType Junction -Path (Join-Path $probeFolder 'cache junction') -Target $cacheTarget | Out-Null
$probeSource = @'
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
public class LaunchProbe {
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetProcessMitigationPolicy(IntPtr process, int policy, out uint flags, UIntPtr length);
    public static void Main(string[] args) {
        uint flags;
        if (!GetProcessMitigationPolicy(GetCurrentProcess(), 16, out flags, (UIntPtr)4)) throw new Exception("Could not read mitigation policy");
        string result = "guard=" + flags + "\n";
        try {
            string file = Path.Combine(args[1], Guid.NewGuid().ToString("N") + ".txt");
            File.WriteAllText(file, "junction access");
            if (File.ReadAllText(file) != "junction access") throw new Exception("Read mismatch");
            File.Delete(file);
            result += "write=ok\n";
        } catch (IOException e) { result += "write_error=" + (e.HResult & 65535) + "\n"; }
        if (args.Length == 2) {
            var child = new ProcessStartInfo(Process.GetCurrentProcess().MainModule.FileName,
                "\"" + args[0] + ".child\" \"" + args[1] + "\" child");
            child.UseShellExecute = false;
            child.CreateNoWindow = true;
            using (var process = Process.Start(child)) { process.WaitForExit(); }
        }
        File.WriteAllText(args[0], result + "complete\n");
    }
}
'@
$sourcePath = Join-Path $probeFolder 'LaunchProbe.cs'
Set-Content -LiteralPath $sourcePath -Value $probeSource -Encoding UTF8
$compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
& $compiler /nologo /target:winexe ('/out:' + (Join-Path $probeFolder 'LaunchProbe.exe')) $sourcePath
if ($LASTEXITCODE -ne 0) { throw 'Could not compile the launch probe' }
$previousProbeDir = $env:DNW_LAUNCH_PROBE_DIR
try {
    $env:DNW_LAUNCH_PROBE_DIR = $probeFolder
    cargo test --release --locked --bin engine-workspace --target-dir $TargetDir 'desktop_launch::tests::desktop_handoff_does_not_inherit_installer_restrictions' -- --ignored --exact --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Post-install desktop handoff regression failed' }
    Get-Content (Join-Path $probeFolder 'direct.txt'),(Join-Path $probeFolder 'desktop.txt'),(Join-Path $probeFolder 'desktop.txt.child')
} finally {
    $env:DNW_LAUNCH_PROBE_DIR = $previousProbeDir
}
