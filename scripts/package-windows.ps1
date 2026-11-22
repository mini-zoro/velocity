# Build and stage unsigned Windows x64 release artifacts.
[CmdletBinding()]
param(
    [string] $OutputDirectory = 'target/package-windows',
    [switch] $SkipBuild,
    [switch] $PortableOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$target = 'x86_64-pc-windows-msvc'
$stage = $null

function Copy-ResourceTree([string] $Source, [string] $Destination, [switch] $LanguageCorpus) {
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    foreach ($entry in Get-ChildItem -LiteralPath $Source -Force) {
        if ($entry.Name.StartsWith('.') -or $entry.Name -eq 'tests') { continue }
        if ($LanguageCorpus -and ($entry.Name -eq 'Default' -or $entry.Extension -in
                '.sublime-keymap', '.sublime-menu', '.sublime-theme', '.sublime-color-scheme')) {
            continue
        }
        if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Unexpected link in resources: $($entry.FullName)"
        }
        $dest = Join-Path $Destination $entry.Name
        if ($entry.PSIsContainer) {
            Copy-ResourceTree $entry.FullName $dest -LanguageCorpus:$LanguageCorpus
        } else {
            Copy-Item -LiteralPath $entry.FullName -Destination $dest
        }
    }
}

function Assert-Executable([string] $Path, [int] $Subsystem, [string] $Version) {
    $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($Path))
    try {
        if ($reader.ReadUInt16() -ne 0x5a4d) { throw "Missing MZ header: $Path" }
        $reader.BaseStream.Position = 0x3c
        $reader.BaseStream.Position = $reader.ReadUInt32()
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
            throw "Expected an x64 PE image: $Path"
        }
        $reader.BaseStream.Position += 18
        $optional = $reader.BaseStream.Position
        if ($reader.ReadUInt16() -ne 0x20b) { throw "Expected PE32+: $Path" }
        $reader.BaseStream.Position = $optional + 68
        if ($reader.ReadUInt16() -ne $Subsystem) { throw "Wrong Windows subsystem: $Path" }
    } finally {
        $reader.Dispose()
    }
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($Path)
    if ($info.ProductName -ne 'Velocity' -or $info.ProductVersion -ne $Version) {
        throw "Missing or incorrect version resource: $Path"
    }
}

Push-Location $repo
try {
    $meta = cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $meta = $meta | ConvertFrom-Json
    $version = ($meta.packages | Where-Object { $_.name -eq 'velocity' }).version
    if ($version -notmatch '^(\d+\.\d+\.\d+)([-+][0-9A-Za-z.+-]+)?$') { throw 'Invalid Cargo version' }
    $numericVersion = "$($Matches[1]).0"
    if ($env:GITHUB_REF_TYPE -eq 'tag' -and $env:GITHUB_REF_NAME -ne "v$version") {
        throw "Tag $env:GITHUB_REF_NAME does not match Cargo package version $version"
    }
    if (-not $SkipBuild) {
        cargo build --locked --release --target $target -p velocity --bins
        if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
    }

    $release = Join-Path $meta.target_directory "$target/release"
    $packaging = Join-Path $repo 'crates/velocity/packaging'
    New-Item -ItemType Directory -Force -Path $output | Out-Null
    $stage = Join-Path $output (".package-" + [Guid]::NewGuid().ToString('N'))
    $payload = Join-Path $stage 'Velocity'
    New-Item -ItemType Directory -Force -Path $payload | Out-Null
    Copy-Item -LiteralPath (Join-Path $release 'velocity.exe'), (Join-Path $release 'vel.exe') -Destination $payload
    $gui = Join-Path $payload 'velocity.exe'
    $cli = Join-Path $payload 'vel.exe'
    Assert-Executable $gui 2 $version
    Assert-Executable $cli 3 $version
    $reportedVersion = & $cli --version
    if ($LASTEXITCODE -ne 0 -or $reportedVersion -ne "Velocity $version") {
        throw 'Packaged console executable did not report its version'
    }

    $resources = Join-Path $payload 'Resources'
    Copy-ResourceTree 'crates/velocity/resources' $resources
    Copy-ResourceTree 'third_party/sublime-packages' (Join-Path $resources 'Syntaxes') -LanguageCorpus
    Copy-ResourceTree 'crates/vel-spell/dictionaries/en_US' (Join-Path $resources 'Dictionaries/en_US')
    Copy-ResourceTree 'crates/vel-spell/LICENSES' (Join-Path $resources 'LICENSES/vel-spell')
    Copy-Item -LiteralPath 'LICENSE-MIT', 'LICENSE-APACHE' -Destination (Join-Path $resources 'LICENSES')
    Copy-Item -LiteralPath 'crates/velocity/fixtures/ime-and-unicode.rs' -Destination $resources
    Copy-Item -LiteralPath (Join-Path $packaging 'README-Windows.txt') -Destination $payload
    $revision = git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'git rev-parse failed' }
    [ordered]@{ version = $version; target = $target; revision = $revision; signed = $false } |
        ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $resources 'build-info.json')

    $base = "Velocity-$version-windows-x64"
    $zip = Join-Path $stage "$base.zip"
    [IO.Compression.ZipFile]::CreateFromDirectory($payload, $zip, [IO.Compression.CompressionLevel]::Optimal, $true)
    $archive = [IO.Compression.ZipFile]::OpenRead($zip)
    try {
        foreach ($item in @(
            'velocity.exe', 'vel.exe', 'README-Windows.txt', 'Resources/build-info.json',
            'Resources/Packages/Default/Default (Windows).sublime-keymap',
            'Resources/Packages/Default/Localization/zh-Hans.sublime-localization',
            'Resources/Packages/Theme - Velocity/Velocity.sublime-theme',
            'Resources/Packages/Color Scheme - Velocity/Velocity Dark.sublime-color-scheme',
            'Resources/Syntaxes/LICENSE', 'Resources/Syntaxes/Rust/Rust.sublime-syntax',
            'Resources/Dictionaries/en_US/LICENSE', 'Resources/LICENSES/vel-spell/NOTICE.md',
            'Resources/LICENSES/LICENSE-MIT', 'Resources/LICENSES/LICENSE-APACHE'
        )) {
            $entry = $archive.GetEntry("Velocity/$item")
            if ($null -eq $entry -or $entry.Length -eq 0) { throw "ZIP is missing $item" }
        }
    } finally {
        $archive.Dispose()
    }
    $artifacts = @($zip)

    if (-not $PortableOnly) {
        $iscc = Get-Command ISCC.exe -ErrorAction SilentlyContinue
        $compiler = if ($iscc) { $iscc.Source } else {
            Join-Path ([Environment]::GetEnvironmentVariable('ProgramFiles(x86)')) 'Inno Setup 6/ISCC.exe'
        }
        if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) {
            throw 'Inno Setup 6 (ISCC.exe) is required to build the installer. Use -PortableOnly for a ZIP only.'
        }
        $innoArgs = @("/DVersion=$version", "/DNumericVersion=$numericVersion", "/DPayload=$payload",
            "/O$stage", "/F$base-setup", (Join-Path $packaging 'velocity.iss'))
        & $compiler @innoArgs
        if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
        $setup = Join-Path $stage "$base-setup.exe"
        if (-not (Test-Path -LiteralPath $setup -PathType Leaf)) { throw 'Missing installer output' }
        $artifacts += $setup
    }

    $sums = Join-Path $stage 'SHA256SUMS'
    $artifacts | ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $([IO.Path]::GetFileName($_))"
    } | Set-Content -Encoding ascii $sums
    foreach ($artifact in $artifacts + $sums) {
        Move-Item -LiteralPath $artifact -Destination $output -Force
    }
} finally {
    Pop-Location
    if ($stage -and (Test-Path -LiteralPath $stage)) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
