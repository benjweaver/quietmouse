# Installs the latest quietmouse release for your user, with no admin rights.
# The programs go in %LOCALAPPDATA%\Programs\quietmouse, which is added to your
# PATH, and quietmouse starts now and whenever you sign in.
#
#   irm https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/windows/install.ps1 | iex
#
# update.ps1 and uninstall.ps1 beside it run this with -Update or -Uninstall.
# Updating does nothing but make sure quietmouse is running when the latest
# release is already installed, and refuses when quietmouse isn't installed.
# Uninstalling keeps your config.
#
# Written for Windows PowerShell 5.1, which every Windows 10 and 11 has.

param([switch]$Uninstall, [switch]$Update)

# Everything runs inside a script block so that `irm | iex` leaves nothing behind
# in your session, and a failure throws rather than closing your terminal.
& {
    param([bool]$Uninstall, [bool]$Update)
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue' # the progress bar slows downloads right down
    $repo = 'benjweaver/quietmouse'
    $dir = Join-Path $env:LOCALAPPDATA 'Programs\quietmouse'
    $exe = Join-Path $dir 'quietmouse.exe'

    function Set-UserPath([bool]$present) {
        $entries = @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' |
                Where-Object { $_ -and $_.TrimEnd('\') -ne $dir })
        if ($present) { $entries += $dir }
        [Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), 'User')
    }

    if ($Uninstall) {
        if (Test-Path $exe) {
            & $exe autostart off
            & $exe stop
        }
        Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
        Set-UserPath $false
        Write-Host "quietmouse is removed. Your config in $env:LOCALAPPDATA\quietmouse is still there."
        return
    }
    if ($Update -and -not (Test-Path $exe)) {
        throw "quietmouse isn't installed in $dir. Install it with: irm https://raw.githubusercontent.com/$repo/main/packaging/windows/install.ps1 | iex"
    }

    # PROCESSOR_ARCHITEW6432 is set when a 32-bit PowerShell runs on 64-bit Windows.
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    $flavour = switch ($arch) {
        'AMD64' { 'windows-x86_64' }
        'ARM64' { 'windows-arm64' }
        default { throw "quietmouse has no build for $arch Windows" }
    }

    # Windows PowerShell 5.1 doesn't offer TLS 1.2 on its own, and GitHub needs it.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -UseBasicParsing
    $tag = $release.tag_name
    if (Test-Path $exe) {
        $current = (& $exe --version) -replace '^quietmouse\s+', ''
        if ("v$current" -eq $tag) {
            Write-Host "quietmouse $current is already the latest release."
            # Still make sure it's registered and running, which is the one thing
            # someone running this again may be hoping to fix.
            & $exe autostart on
            return
        }
        Write-Host "Updating quietmouse $current to $tag"
    }
    $zipName = "quietmouse-$tag-$flavour.zip"
    $base = "https://github.com/$repo/releases/download/$tag"

    $work = Join-Path ([IO.Path]::GetTempPath()) "quietmouse-install-$([guid]::NewGuid())"
    New-Item -ItemType Directory $work | Out-Null
    try {
        Write-Host "Downloading quietmouse $tag for $flavour"
        $zip = Join-Path $work $zipName
        Invoke-WebRequest "$base/$zipName" -OutFile $zip -UseBasicParsing
        # Saved and read back rather than taken from .Content, which PowerShell 5.1
        # hands over as bytes for a file served without a text type.
        $sums = Join-Path $work 'SHA256SUMS'
        Invoke-WebRequest "$base/SHA256SUMS" -OutFile $sums -UseBasicParsing
        # The file lists "<hash>  <name>", one per line.
        $line = Get-Content $sums | Where-Object { $_ -match "\s\*?$([regex]::Escape($zipName))\s*$" } | Select-Object -First 1
        if (-not $line) { throw "SHA256SUMS for $tag doesn't list $zipName" }
        $expected = ($line -split '\s+')[0]
        $actual = (Get-FileHash $zip -Algorithm SHA256).Hash
        if ($actual -ne $expected) { throw "$zipName doesn't match its checksum; nothing was installed" }

        # The zip holds a single folder named quietmouse.
        Expand-Archive $zip -DestinationPath $work
        $new = Join-Path $work 'quietmouse'
        Get-ChildItem $new | Unblock-File

        # Stop a running copy, wherever it was installed, so the new one starts in
        # its place and its files aren't locked.
        & (Join-Path $new 'quietmouse.exe') stop | Out-Null

        New-Item -ItemType Directory $dir -Force | Out-Null
        Copy-Item (Join-Path $new '*') $dir -Force
        Write-Host "Installed quietmouse and quietmoused in $dir"
    }
    finally {
        Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    Set-UserPath $true

    # `config` notes on stderr that the file doesn't exist yet, and PowerShell 5.1
    # turns redirected stderr into an error that 'Stop' would make fatal.
    $config = & {
        $ErrorActionPreference = 'Continue'
        & $exe config 2>$null | Select-Object -First 1
    }
    if (-not (Test-Path $config)) {
        & $exe config --init
        Write-Host "Edit $config to change settings."
    }
    & $exe autostart on
    Write-Host 'Quit Logi Options+ if it is running; it configures the same settings.'
    Write-Host 'Open a new terminal to run quietmouse from anywhere.'
} $Uninstall.IsPresent $Update.IsPresent
