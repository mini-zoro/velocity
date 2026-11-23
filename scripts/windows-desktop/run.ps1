[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $PackageDirectory,
    [Parameter(Mandatory)] [string] $ResultDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-That([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}

function Wait-Until([string] $Description, [scriptblock] $Probe, [int] $Seconds = 10) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    do {
        $value = & $Probe
        if ($value) { return $value }
        Start-Sleep -Milliseconds 80
    } while ($timer.Elapsed.TotalSeconds -lt $Seconds)
    throw "Timed out after ${Seconds}s: $Description"
}

function Focus-Window([IntPtr] $Window) {
    $null = Wait-Until 'foreground window' {
        [VelocityDesktop.Native]::Focus($Window)
        [VelocityDesktop.Native]::Foreground -eq $Window
    } 4
}

function Write-Utf8([string] $Path, [AllowEmptyString()] [string] $Value) {
    [IO.File]::WriteAllText($Path, $Value, [Text.UTF8Encoding]::new($false))
}

function Write-Json([string] $Path, $Value) {
    Write-Utf8 $Path (ConvertTo-Json -InputObject $Value -Depth 16)
}

function Read-SharedText([string] $Path) {
    # Velocity appends telemetry while it is live and atomically replaces session saves.
    $stream = [IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite, Delete')
    $reader = [IO.StreamReader]::new($stream, [Text.Encoding]::UTF8)
    try { $reader.ReadToEnd() } finally { $reader.Dispose() }
}

function Quote-Argument([AllowEmptyString()] [string] $Text) {
    if ($Text -and $Text -notmatch '[\s"]') { return $Text }
    # CommandLineToArgvW: double slashes before a quote or the closing delimiter.
    '"' + ($Text -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
}

function Start-Native([string] $File, [string] $Label, [string[]] $Arguments, [string] $RawArguments) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $File
    $info.WorkingDirectory = $ctx.CaseRoot
    $info.Arguments = if ($RawArguments) { $RawArguments } else {
        ($Arguments | ForEach-Object { Quote-Argument $_ }) -join ' '
    }
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = [Text.Encoding]::UTF8
    $info.StandardErrorEncoding = [Text.Encoding]::UTF8
    $process = [Diagnostics.Process]::Start($info)
    $child = [pscustomobject]@{ Process = $process; Label = $Label; Command = $info.Arguments;
        Out = $process.StandardOutput.ReadToEndAsync(); Err = $process.StandardError.ReadToEndAsync() }
    $ctx.Children.Add($child)
    $child
}

function Complete-Native($Child, [int] $Timeout = 12) {
    Assert-That ($Child.Process.WaitForExit($Timeout * 1000)) "$($Child.Label) did not exit within ${Timeout}s"
    Assert-That ($Child.Out.Wait(2000) -and $Child.Err.Wait(2000)) "$($Child.Label) did not close its output streams"
    Assert-That ($Child.Process.ExitCode -eq 0) "$($Child.Label) exited $($Child.Process.ExitCode): $($Child.Err.Result.Trim())"
    $Child.Out.Result
}

function Require-Desktop {
    Assert-That ($null -eq $ctx.DesktopError) "Interactive desktop unavailable: $($ctx.DesktopError)"
}

function Get-SuiteProcesses([switch] $GuiOnly) {
    $names = if ($GuiOnly) { 'velocity' } else { 'velocity', 'vel' }
    $paths = @($ctx.Gui, $ctx.Cli, $ctx.InstalledGui, $ctx.InstalledCli) | Where-Object { $_ }
    Get-Process -Name $names -ErrorAction SilentlyContinue | Where-Object { $_.Path -in $paths }
}

function Get-EditorWindows([uint32] $ProcessId = 0) {
    if (-not $ProcessId) {
        foreach ($process in @(Get-SuiteProcesses -GuiOnly)) { Get-EditorWindows $process.Id }
        return
    }
    [VelocityDesktop.Native]::Windows($ProcessId) | Where-Object {
        [VelocityDesktop.Native]::Class($_) -eq 'VelocityEditorWindow'
    }
}

function Save-Evidence([IntPtr] $Window, [string] $Step, [IntPtr[]] $Include = @()) {
    Require-Desktop
    $desktop = [VelocityDesktop.Native]::DesktopBounds()
    $area = if ($Window -eq [IntPtr]::Zero) { $desktop } else { [VelocityDesktop.Native]::Outer($Window) }
    if ($Window -ne [IntPtr]::Zero) {
        foreach ($extra in @($Include) + @([VelocityDesktop.Native]::Menus($Window))) {
            $other = [VelocityDesktop.Native]::Outer($extra)
            if (-not $other.IsEmpty) { $area = [Drawing.Rectangle]::Union($area, $other) }
        }
    }
    $area = [Drawing.Rectangle]::Intersect($area, $desktop)
    $source = if ($Window -eq [IntPtr]::Zero) { 'desktop' } else { 'screen crop' }
    try { $image = [VelocityDesktop.Native]::Screen($area) }
    catch {
        if ($Window -eq [IntPtr]::Zero -or $Include.Count -or [VelocityDesktop.Native]::InMenu($Window)) { throw }
        $image = [VelocityDesktop.Native]::WindowImage($Window)
        $source = 'PrintWindow fallback (not compositor evidence)'
    }
    $name = "$($ctx.Check.key)-$Step.png"
    try { $image.Save((Join-Path $ctx.Screens $name), [Drawing.Imaging.ImageFormat]::Png) }
    finally { $image.Dispose() }
    $ctx.Check.screenshots.Add($name)
    $ctx.Check.capture_sources[$name] = $source
}

function Get-Frame([IntPtr] $Window, [ValidateSet('client', 'top', 'bottom', 'left', 'text')] [string] $Region = 'client') {
    $r = [Drawing.Rectangle]::Intersect([VelocityDesktop.Native]::Client($Window), [VelocityDesktop.Native]::DesktopBounds())
    $area = switch ($Region) {
        'top'    { [Drawing.Rectangle]::new($r.X + [int]($r.Width * .15), $r.Y, [int]($r.Width * .70), [int]($r.Height * .42)) }
        'bottom' { [Drawing.Rectangle]::new($r.X, $r.Y + [int]($r.Height * .68), $r.Width, [int]($r.Height * .29)) }
        'left'   { [Drawing.Rectangle]::new($r.X, $r.Y + [int]($r.Height * .12), [int]($r.Width * .23), [int]($r.Height * .70)) }
        'text'   { [Drawing.Rectangle]::new($r.X + [int]($r.Width * .10), $r.Y + [int]($r.Height * .10), [int]($r.Width * .55), [int]($r.Height * .65)) }
        default  { $r }
    }
    [VelocityDesktop.Native]::Screen($area)
}

function Measure-Frame($Image) {
    $hist = [VelocityDesktop.Native]::Histogram($Image)
    $top = $hist.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 1
    [pscustomobject]@{ colors=$hist.Count; dominant=('#{0:X6}' -f $top.Key);
        non_background=($Image.Width * $Image.Height - $top.Value) }
}

function Wait-Frame([IntPtr] $Window, [int] $Seconds = 20) {
    Wait-Until 'non-blank composited client frame' {
        if ([VelocityDesktop.Native]::Foreground -ne $Window) { return }
        $image = Get-Frame $Window
        try {
            $stats = Measure-Frame $image
            if ($stats.colors -ge 8 -and $stats.non_background -ge 500) { $stats }
        } finally { $image.Dispose() }
    } $Seconds
}

function Wait-Changed([IntPtr] $Window, $Before, [string] $Region, [int] $Minimum = 400) {
    Wait-Until "rendered $Region changes by at least $Minimum pixels" {
        $image = Get-Frame $Window $Region
        try {
            if ($Before.Size -eq $image.Size) {
                $pixels = [VelocityDesktop.Native]::Difference($Before, $image)
                if ($pixels -ge $Minimum) { $pixels }
            }
        } finally { $image.Dispose() }
    }
}

function Wait-Title([IntPtr] $Window, [string] $Part, [int] $Seconds = 10) {
    Wait-Until "title containing '$Part'" {
        $title = [VelocityDesktop.Native]::Title($Window)
        if ($title.Contains($Part)) { $title }
    } $Seconds
}

function Set-Preferences([System.Collections.IDictionary] $Settings) {
    $Settings['ui_language'] = if ($Settings.Contains('ui_language')) { $Settings['ui_language'] } else { 'en' }
    [IO.Directory]::CreateDirectory((Split-Path -Parent $ctx.Preferences)) | Out-Null
    Write-Json $ctx.Preferences $Settings
}

function Start-Editor([string] $Label = 'gui', [string[]] $Locations = @(), [switch] $ExplicitProfile,
        [string] $Executable = $ctx.Gui) {
    Require-Desktop
    Assert-That (Test-Path -LiteralPath $Executable -PathType Leaf) "Missing GUI executable: $Executable ($($ctx.PackageError))"
    $arguments = @('--width', '850', '--height', '550', '--output', (Join-Path $ctx.Logs "$Label.jsonl"))
    if ($ExplicitProfile) { $arguments += @('--data-dir', $ctx.Profile) }
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $child = Start-Native $Executable $Label ($arguments + $Locations)
    $window = Wait-Until "visible editor window (pid $($child.Process.Id))" {
        if ($child.Process.HasExited) { throw "Velocity exited before showing a window (code $($child.Process.ExitCode)); see $Label logs" }
        Get-EditorWindows $child.Process.Id | Select-Object -First 1
    } 30
    $shown = $timer.ElapsedMilliseconds
    $child.Process.Refresh()
    $startup = [ordered]@{ pid=$child.Process.Id; visible_ms=$shown; first_nonblank_ms=$null;
        title=[VelocityDesktop.Native]::Title($window); class=[VelocityDesktop.Native]::Class($window);
        bounds=[string][VelocityDesktop.Native]::Outer($window); dpi=[VelocityDesktop.Native]::GetDpiForWindow($window);
        awareness=[VelocityDesktop.Native]::Awareness($window); input_language=[VelocityDesktop.Native]::InputLanguage($window);
        working_set_bytes=$child.Process.WorkingSet64; pixels=$null }
    $ctx.Check.metrics[$Label] = $startup
    Focus-Window $window
    $startup.pixels = Wait-Frame $window
    $startup.first_nonblank_ms = $timer.ElapsedMilliseconds
    $child.Process.Refresh()
    $startup.working_set_bytes = $child.Process.WorkingSet64
    $window
}

function Start-Helper([string] $Label, [string[]] $Arguments) {
    Assert-That (Test-Path -LiteralPath $ctx.Cli -PathType Leaf) "Missing console executable: $($ctx.Cli)"
    Start-Native $ctx.Cli $Label $Arguments
}

function Open-File([IntPtr] $Window, [string] $File, [string] $Label, [int] $Timeout = 12) {
    $null = Complete-Native (Start-Helper $Label @($File)) $Timeout
    $null = Wait-Title $Window ([IO.Path]::GetFileName($File)) $Timeout
}

function Expect-Bytes([string] $File, [string] $Expected) {
    $hex = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Expected))
    try {
        $null = Wait-Until "exact UTF-8 bytes in $File" {
            try { [Convert]::ToBase64String([IO.File]::ReadAllBytes($File)) -ceq $hex }
            catch [IO.IOException] { $false }
        }
    } catch {
        if (Test-Path -LiteralPath $File -PathType Leaf) {
            $actual = [IO.File]::ReadAllBytes($File)
            Write-Utf8 (Join-Path $ctx.Logs ((Split-Path $File -Leaf) + '.actual-base64.txt')) ([Convert]::ToBase64String($actual))
        }
        throw
    }
}

function Read-Editor([IntPtr] $Window, [switch] $SelectionOnly) {
    Focus-Window $Window
    $marker = [Guid]::NewGuid().ToString('N')
    Set-Clipboard -Value $marker
    if (-not $SelectionOnly) { Send-Key $Window @('Ctrl', 'A') }
    Send-Key $Window @('Ctrl', 'C')
    Wait-Until 'editor selection copied to clipboard' {
        $value = Get-Clipboard -Raw
        if ($value -and $value -cne $marker) { $value }
    }
}

function Exit-Editor([IntPtr] $Window) {
    Focus-Window $Window
    Send-Key $Window @('Alt', 'F')
    $null = Wait-Until 'File menu opened' { [VelocityDesktop.Native]::Menus($Window).Length -gt 0 }
    # Exit's Win32 access key works even if this tall File menu overflows the display.
    Send-Key $Window X
    $null = Wait-Until 'all packaged editor processes exit' { @(Get-SuiteProcesses -GuiOnly).Count -eq 0 }
}

function Invoke-Menu([IntPtr] $Window, [string] $AccessKey, [string] $CaptionPattern) {
    Focus-Window $Window
    Send-Key $Window @('Alt', $AccessKey)
    $item = Wait-Until "native menu item $CaptionPattern" {
        Get-PopupItems $Window | Where-Object { (Get-MenuCaption $_.Current.Name) -match $CaptionPattern } | Select-Object -First 1
    }
    $action = $null
    if ($item.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$action)) { $action.Invoke() }
    else {
        $at = $item.GetClickablePoint()
        [VelocityDesktop.Native]::Click([int]$at.X, [int]$at.Y)
    }
}

function Save-FailureEvidence([string] $Step) {
    if ($ctx.DesktopError) { return }
    try {
        $foreground = [VelocityDesktop.Native]::Foreground
        $window = Get-EditorWindows | Select-Object -First 1
        if (-not $window) { $window = [IntPtr]::Zero }
        $dialog = @()
        if ($window -ne [IntPtr]::Zero -and $foreground -ne $window -and
                [VelocityDesktop.Native]::Pid($foreground) -eq [VelocityDesktop.Native]::Pid($window)) { $dialog += $foreground }
        Save-Evidence $window $Step $dialog
    } catch { $ctx.Check.metrics["${Step}_capture_error"] = $_.Exception.Message }
}

function Invoke-Probe([string] $Name, [scriptblock] $Body) {
    $sub = [ordered]@{ name=$Name; status='PASS'; elapsed_ms=0; detail=''; screenshots=@() }
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $from = $ctx.Check.screenshots.Count
    try { & $Body }
    catch {
        $sub.status = 'FAIL'; $sub.detail = $_.Exception.Message; $sub['error_location'] = $_.ScriptStackTrace
        Write-Warning ("$Name : $($sub.detail)")
        Save-FailureEvidence ('failed-' + ($Name.ToLowerInvariant() -replace '[^a-z0-9]+', '-').Trim('-'))
    }
    finally {
        $sub.elapsed_ms = $timer.ElapsedMilliseconds
        $sub.screenshots = @($ctx.Check.screenshots | Select-Object -Skip $from)
        $ctx.Check.steps.Add($sub)
    }
}

function Get-AppEvents([datetime] $Since) {
    try { $events = @(Get-WinEvent -FilterHashtable @{LogName='Application'; StartTime=$Since}) }
    catch {
        if ($_.FullyQualifiedErrorId -notlike 'NoMatchingEventsFound*') { throw }
        $events = @()
    }
    foreach ($event in $events) {
        $xml = $event.ToXml()
        $message = $event.Message
        if ($message -match '(?i)(?<![\w.-])(?:velocity|vel)\.exe\b' -or $xml -match '(?i)(?<![\w.-])(?:velocity|vel)\.exe\b') {
            [ordered]@{ id=$event.Id; record_id=$event.RecordId; provider=$event.ProviderName;
                time_utc=$event.TimeCreated.ToUniversalTime().ToString('o'); level=$event.Level; message=$message; xml=$xml }
        }
    }
}

function Collect-Check([datetime] $Since) {
    # Scope cleanup to children and to the exact two binaries under test, never all processes by name.
    foreach ($child in $ctx.Children) {
        if (-not $child.Process.HasExited) {
            $child.Process.Kill()
            $child | Add-Member -NotePropertyName TerminatedByCleanup -NotePropertyValue $true
            $null = $child.Process.WaitForExit(3000)
        } else { $child | Add-Member -NotePropertyName TerminatedByCleanup -NotePropertyValue $false }
    }
    foreach ($process in @(Get-SuiteProcesses)) {
        if (-not $process.HasExited) { $process.Kill(); $null = $process.WaitForExit(3000) }
    }
    $processes = foreach ($child in $ctx.Children) {
        $out = if ($child.Out.Wait(2000)) { $child.Out.Result } else { '(stream did not close)' }
        $err = if ($child.Err.Wait(2000)) { $child.Err.Result } else { '(stream did not close)' }
        Write-Utf8 (Join-Path $ctx.Logs "$($child.Label).stdout.log") $out
        Write-Utf8 (Join-Path $ctx.Logs "$($child.Label).stderr.log") $err
        [ordered]@{ pid=$child.Process.Id; command=$child.Command; label=$child.Label; exit_code=$child.Process.ExitCode;
            cleanup_terminated=$child.TerminatedByCleanup }
        $child.Process.Dispose()
    }
    Write-Json (Join-Path $ctx.Logs 'processes.json') @($processes)
    $events = @(Get-AppEvents $Since)
    Write-Json (Join-Path $ctx.Logs 'application-events.json') $events
    $reports = @(Get-ChildItem -LiteralPath $ctx.Logs -Recurse -File | Where-Object { $_.Name -match '\.(crash|signal|dmp)(\.seen)?$' })
    $ctx.Check.metrics['crash_reports'] = @($reports | ForEach-Object { $_.Name })
    $ctx.Check.metrics['application_event_ids'] = @($events | ForEach-Object { $_.id })
    $errors = @($events | Where-Object {
        ($null -ne $_.level -and $_.level -in 1,2) -or $_.id -in 1000,1002 -or
        ($_.id -eq 1001 -and $_.message -match 'APPCRASH|AppHang|BEX')
    })
    Assert-That (-not ($reports.Count -or $errors.Count)) "Found $($reports.Count) crash report(s) and $($errors.Count) Windows Application error(s)"
}

function Invoke-Check([int] $Number, [string] $Id, [string] $Name, [scriptblock] $Body) {
    $key = '{0:D2}-{1}' -f $Number, $Id
    $start = Get-Date
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $check = [ordered]@{ key=$key; name=$Name; status='PASS'; elapsed_ms=0; detail=''; error_location='';
        logs="logs/$key"; metrics=[ordered]@{}; screenshots=[Collections.Generic.List[string]]::new();
        capture_sources=[ordered]@{}; steps=[Collections.Generic.List[object]]::new(); skip_reason='' }
    $ctx.Check = $check
    $ctx.CaseRoot = Join-Path $ctx.Root $key
    $ctx.Profile = Join-Path $ctx.CaseRoot 'profile'
    $ctx.Preferences = Join-Path $ctx.Profile 'Packages/User/Preferences.sublime-settings'
    $ctx.Logs = Join-Path $results $check.logs
    $ctx.Children = [Collections.Generic.List[object]]::new()
    $env:VELOCITY_DATA_DIR = $ctx.Profile
    $env:VELOCITY_LOG_DIR = $ctx.Logs
    [IO.Directory]::CreateDirectory($ctx.Logs) | Out-Null
    [IO.Directory]::CreateDirectory($ctx.CaseRoot) | Out-Null
    Write-Host "Running $key : $Name"
    try {
        Set-Preferences @{}
        & $Body
        $failed = @($check.steps | Where-Object { $_.status -eq 'FAIL' })
        if ($failed.Count) { throw ('Failed: ' + (($failed | ForEach-Object { $_.name }) -join ', ')) }
        if ($check.skip_reason) { $check.status = 'SKIP'; $check.detail = $check.skip_reason }
    } catch {
        $check.status = 'FAIL'; $check.detail = $_.Exception.Message; $check.error_location = $_.ScriptStackTrace
        Save-FailureEvidence 'failure'
    } finally {
        try { Collect-Check $start }
        catch { $check.status = 'FAIL'; $check.detail = "$($check.detail) Diagnostics: $($_.Exception.Message)".Trim() }
        $check.elapsed_ms = $timer.ElapsedMilliseconds
        $report.checks.Add($check)
        Write-Host "$($check.status) $key in $($check.elapsed_ms)ms $($check.detail)"
        Write-Report
    }
}

function Format-RowText([string] $Value) { $Value -replace '\|', '\|' -replace '[\r\n]+', ' ' }

function Write-Report {
    Write-Json (Join-Path $results 'report.json') $report
    $lines = @('# Velocity Windows desktop', '', "Runner: $($report.image). Commit: $($env:GITHUB_SHA).", '')
    if ($ctx.DesktopError) { $lines += 'Desktop preflight: ' + $ctx.DesktopError; $lines += '' }
    if ($ctx.PackageError) { $lines += 'Package preflight: ' + $ctx.PackageError; $lines += '' }
    $lines += '| Check | Result | ms | Screenshots | Detail |', '| --- | --- | ---: | --- | --- |'
    foreach ($row in $report.checks) {
        $lines += "| $($row.key): $($row.name) | $($row.status) | $($row.elapsed_ms) | $($row.screenshots -join ', ') | $(Format-RowText $row.detail) |"
        foreach ($step in $row.steps) {
            $lines += "| $(Format-RowText $step.name) | $($step.status) | $($step.elapsed_ms) | $($step.screenshots -join ', ') | $(Format-RowText $step.detail) |"
        }
    }
    $lines += '', 'PNGs are in `screenshots/`; telemetry, crash reports, CLI output and Application events are under `logs/`.', ''
    Write-Utf8 (Join-Path $results 'summary.md') ($lines -join "`n")
}

$results = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($ResultDirectory)
$package = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($PackageDirectory)
$suiteStart = Get-Date
$fixture = ConvertFrom-Json ([IO.File]::ReadAllText((Join-Path $PSScriptRoot 'fixtures.json')))
$temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$root = Join-Path $temp ('Velocity ' + [char]0x6d4b + [char]0x8bd5 + ' ' + [Guid]::NewGuid().ToString('N').Substring(0,8))
$screens = Join-Path $results 'screenshots'
[IO.Directory]::CreateDirectory($screens) | Out-Null
[IO.Directory]::CreateDirectory($root) | Out-Null
$ctx = @{ Root=$root; Screens=$screens; DesktopError=$null; PackageError=$null; Setup=$null;
    Gui=(Join-Path $root 'Velocity/velocity.exe'); Cli=(Join-Path $root 'Velocity/vel.exe'); InstalledGui=$null; InstalledCli=$null }
$report = [ordered]@{ schema_version=1; image=$env:ImageOS; image_version=$env:ImageVersion; revision=$env:GITHUB_SHA;
    os=[Environment]::OSVersion.VersionString; powershell=$PSVersionTable.PSVersion.ToString(); desktop=$null;
    package=$null; checks=[Collections.Generic.List[object]]::new() }
try {
    Import-Module (Join-Path $PSScriptRoot 'Desktop.psm1') -DisableNameChecking -Force
    $session = [Diagnostics.Process]::GetCurrentProcess().SessionId
    Assert-That ([Environment]::UserInteractive -and $session -ne 0) 'Runner is not in an interactive user session'
    Assert-That ([Threading.Thread]::CurrentThread.GetApartmentState() -eq 'STA') 'Run the driver in an STA PowerShell host'
    $desktop = [VelocityDesktop.Native]::InputDesktop()
    $image = [VelocityDesktop.Native]::Screen([VelocityDesktop.Native]::DesktopBounds())
    $image.Dispose()
    $report.desktop = [ordered]@{ session_id=$session; name=$desktop; bounds=[string][VelocityDesktop.Native]::DesktopBounds() }
} catch { $ctx.DesktopError = $_.Exception.Message; $report.desktop = @{ error=$ctx.DesktopError } }
try {
    $zip = @(Get-ChildItem -LiteralPath $package -Filter 'Velocity-*-windows-x64.zip' -File)
    $setups = @(Get-ChildItem -LiteralPath $package -Filter 'Velocity-*-windows-x64-setup.exe' -File)
    Assert-That ($zip.Count -eq 1 -and $setups.Count -eq 1) 'Expected exactly one portable ZIP and one installer'
    $sums = [IO.File]::ReadAllLines((Join-Path $package 'SHA256SUMS'))
    foreach ($asset in $zip[0], $setups[0]) {
        $hash = (Get-FileHash -LiteralPath $asset.FullName -Algorithm SHA256).Hash
        Assert-That ("$hash  $($asset.Name)" -in $sums) "SHA256 mismatch for $($asset.Name)"
    }
    $ctx.Setup = $setups[0].FullName
    Expand-Archive -LiteralPath $zip[0].FullName -DestinationPath $root
    $report.package = ConvertFrom-Json ([IO.File]::ReadAllText((Join-Path $root 'Velocity/Resources/build-info.json')))
} catch { $ctx.PackageError = $_.Exception.Message; $report.package = @{ error=$ctx.PackageError } }

. (Join-Path $PSScriptRoot 'Editor.ps1')
. (Join-Path $PSScriptRoot 'System.ps1')

Invoke-Check 1 launch       'Portable launch, DPI and first frame' { Test-Launch }
Invoke-Check 2 rendering    'Syntax highlighting and CJK' { Test-Rendering }
Invoke-Check 3 typing       'Scan codes, Unicode, navigation, undo and redo' { Test-Typing }
Invoke-Check 4 save-as      'Native Save As, Unicode path and CRLF' { Test-SaveAs }
Invoke-Check 5 menus        'Native menus in English and Simplified Chinese' { Test-Menus }
Invoke-Check 6 shortcuts    'Windows editor shortcuts' { Test-Shortcuts }
Invoke-Check 7 console      'Console shell, forwarding, wait, stdin and cold start' { Test-Console }
Invoke-Check 8 session      'Multiple windows, move/resize, exit and restore' { Test-Session }
Invoke-Check 9 clipboard    'CJK clipboard round trip' { Test-ClipboardRoundTrip }
Invoke-Check 10 project     'CLI folder and sidebar keyboard navigation' { Test-Project }
Invoke-Check 11 settings    'Paired JSON and graphical settings' { Test-Settings }
Invoke-Check 12 large-file  '50 MiB text, first frame, scroll and working set' { Test-LargeFile }
Invoke-Check 13 installer   'Per-user install, launch and uninstall' { Test-Installer }
Invoke-Check 14 recovery    'Forced termination and dirty-buffer recovery' { Test-Recovery }
Invoke-Check 15 ime         'Optional Microsoft Pinyin composition' { Test-Pinyin }

# Check once more for WER events posted asynchronously after a process exited.
$suiteErrors = @()
try {
    $allEvents = @(Get-AppEvents $suiteStart)
    Write-Json (Join-Path $results 'application-events.json') $allEvents
    $suiteErrors = @($allEvents | Where-Object { $_.level -in 1,2 -or $_.id -in 1000,1002 -or
        ($_.id -eq 1001 -and $_.message -match 'APPCRASH|AppHang|BEX') })
} catch { $suiteErrors = @($_.Exception.Message) }
$report['late_event_check'] = @{ errors=$suiteErrors.Count; artifact='application-events.json' }
Write-Report
if ($suiteErrors.Count) {
    [IO.File]::AppendAllText((Join-Path $results 'summary.md'), "`nApplication log scan found $($suiteErrors.Count) error(s) across the run. See application-events.json.`n")
}
if ($env:GITHUB_STEP_SUMMARY) {
    [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY, [IO.File]::ReadAllText((Join-Path $results 'summary.md')))
}
$failures = @($report.checks | Where-Object { $_.status -eq 'FAIL' }).Count
if ($failures -or $suiteErrors.Count) { throw "$failures desktop check(s) failed; $($suiteErrors.Count) Application log error(s). See the desktop artifact." }
