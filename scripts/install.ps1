# akua install script for Windows — `irm https://akua.cnap.tech/install.ps1 | iex`
#
# Downloads a prebuilt akua.exe from GitHub Releases into
# $env:AKUA_INSTALL\bin (default: $env:USERPROFILE\.akua\bin) and prints
# the PATH line to paste.
#
# We don't mutate the user's PATH or Registry — printing the env var
# line is cleaner than modifying user state.

$ErrorActionPreference = 'Stop'

function Info($msg)    { Write-Host "→ $msg" -ForegroundColor Blue }
function Success($msg) { Write-Host "✓ $msg" -ForegroundColor Green }
function Die($msg)     { Write-Host "✗ $msg" -ForegroundColor Red; exit 1 }

# ---------------------------------------------------------------------------
# Target detection
# ---------------------------------------------------------------------------

# PowerShell's $env:PROCESSOR_ARCHITECTURE returns "x86" on 32-bit
# PowerShell even on 64-bit Windows. Use the PROCESSOR_ARCHITEW6432
# fallback for the real machine arch.
$arch = $env:PROCESSOR_ARCHITEW6432
if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }

$triple = switch ($arch) {
    'AMD64' { 'x86_64-pc-windows-msvc' }
    'x86_64' { 'x86_64-pc-windows-msvc' }
    # aarch64-pc-windows-msvc builds aren't shipped yet. Users on ARM64
    # Windows get a clear error rather than a silently-broken binary.
    'ARM64' { Die "ARM64 Windows not yet supported. File an issue at https://github.com/cnap-tech/akua/issues" }
    default { Die "unsupported Windows arch: $arch" }
}

# ---------------------------------------------------------------------------
# Version resolution
# ---------------------------------------------------------------------------

# First positional arg: optional version. `v0.1.0`, `0.1.0`, or
# `akua-v0.1.0` all accepted.
$requestedVersion = $args[0]

function Resolve-Version($v) {
    if ($v) {
        $v = $v -replace '^akua-',''
        if ($v -notmatch '^v') { $v = "v$v" }
        return $v
    }
    # Use the GitHub /releases/latest redirect to discover the tag name.
    # -MaximumRedirection 0 means: stop at the first redirect and read
    # its Location header, rather than actually following it.
    $resp = try {
        Invoke-WebRequest -Uri 'https://github.com/cnap-tech/akua/releases/latest' `
            -MaximumRedirection 0 -ErrorAction SilentlyContinue
    } catch {
        $_.Exception.Response
    }
    $loc = $resp.Headers.Location
    if (-not $loc) { Die "could not resolve latest version from GitHub" }
    # URL ends with .../tag/akua-vX.Y.Z
    ($loc -split '/tag/akua-')[-1]
}

$version = Resolve-Version $requestedVersion

# ---------------------------------------------------------------------------
# Download + install
# ---------------------------------------------------------------------------

$base = if ($env:AKUA_DOWNLOAD_BASE) { $env:AKUA_DOWNLOAD_BASE } else { 'https://github.com' }
$asset = "akua-$version-$triple.zip"
$url = "$base/cnap-tech/akua/releases/download/akua-$version/$asset"

$installRoot = if ($env:AKUA_INSTALL) { $env:AKUA_INSTALL } else { Join-Path $env:USERPROFILE '.akua' }
$binDir = Join-Path $installRoot 'bin'

Info "downloading akua $version ($triple)"
Info "  from  $url"
Info "  to    $binDir\akua.exe"

New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$tmp = Join-Path $env:TEMP ("akua-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    $zipPath = Join-Path $tmp 'akua.zip'
    Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing `
        -ErrorAction Stop

    Expand-Archive -Path $zipPath -DestinationPath $tmp -Force

    $exeSrc = Join-Path $tmp 'akua.exe'
    if (-not (Test-Path $exeSrc)) {
        Die "archive did not contain akua.exe"
    }
    Move-Item -Path $exeSrc -Destination (Join-Path $binDir 'akua.exe') -Force
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Success "installed akua $version to $binDir\akua.exe"
Write-Host ""

# PATH check — case-insensitive, split on `;`.
$pathParts = $env:PATH -split ';'
if ($pathParts -notcontains $binDir) {
    Info "add to your PATH (persists across sessions):"
    Write-Host ""
    Write-Host "    `$env:PATH = `"$binDir;`$env:PATH`""
    Write-Host "    [Environment]::SetEnvironmentVariable('PATH', `"$binDir;`" + [Environment]::GetEnvironmentVariable('PATH','User'), 'User')"
    Write-Host ""
}

Info "verify:  $binDir\akua.exe --version"
