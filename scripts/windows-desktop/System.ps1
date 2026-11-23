# Desktop integration: shell entry points, session files, large documents, and installer.

function Start-CmdHelper([string] $Label, [string] $Arguments) {
    Start-Native $env:ComSpec $Label -RawArguments ('/D /V:OFF /S /C ""{0}" {1}"' -f $ctx.Cli, $Arguments)
}

function Confirm-StdinClose([IntPtr] $Window, [IntPtr] $Dialog) {
    $null = Wait-Until 'owned save prompt is foreground' { [VelocityDesktop.Native]::Foreground -eq $Dialog }
    # Both native alert implementations use 100 + index for their buttons.
    $button = Wait-Until "native Don't Save button" {
        $control = [VelocityDesktop.Native]::GetDlgItem($Dialog, 101)
        if ($control -ne [IntPtr]::Zero -and [VelocityDesktop.Native]::Class($control) -eq 'Button' -and
                [VelocityDesktop.Native]::ControlText($control) -ceq "Don't Save") { $control }
    } 4
    $area = [VelocityDesktop.Native]::Outer($button)
    Assert-That (-not $area.IsEmpty -and [VelocityDesktop.Native]::Foreground -eq $Dialog) 'The save prompt no longer owns the input target'
    Click-Rectangle $area
    $null = Wait-Until 'stdin save prompt is dismissed' { -not (Get-OwnedDialog $Window '^Velocity$') }
}

function Test-Console {
    $cliLiteral = "'" + $ctx.Cli.Replace("'", "''") + "'"
    $powershell = Join-Path $PSHOME 'powershell.exe'
    foreach ($shellName in 'cmd', 'powershell') {
        foreach ($option in 'help', 'version') {
            Invoke-Probe "$shellName --$option" {
                $label = "$shellName-$option"
                $child = if ($shellName -eq 'cmd') {
                    Start-CmdHelper $label "--$option"
                } else {
                    Start-Native $powershell $label @('-NoProfile', '-NonInteractive', '-Command', "& $cliLiteral --$option; exit `$LASTEXITCODE")
                }
                $output = Complete-Native $child
                $matchesExpected = if ($option -eq 'help') {
                    $output.StartsWith('Usage: vel [options]')
                } else { $output.Trim() -ceq ('Velocity ' + $report.package.version) }
                Assert-That $matchesExpected "$label showed unexpected output: $output"
                Assert-That (@(Get-SuiteProcesses -GuiOnly).Count -eq 0) "$label created a GUI"
            }
        }
    }
    $location = Join-Path $ctx.CaseRoot 'line location.txt'
    $wait = Join-Path $ctx.CaseRoot 'wait.txt'
    $cold = Join-Path $ctx.CaseRoot 'cold.txt'
    Write-Utf8 $location "line one`r`nline two`r`nabcde"
    Write-Utf8 $wait 'wait on this sheet'
    Write-Utf8 $cold 'opened by the cold helper'
    Invoke-Probe 'Warm GUI using --data-dir over a different environment profile' {
        $env:VELOCITY_DATA_DIR = Join-Path $ctx.CaseRoot 'decoy-profile'
        try { $ctx.WarmWindow = Start-Editor 'explicit-profile' -ExplicitProfile -LogInput }
        finally { $env:VELOCITY_DATA_DIR = $ctx.Profile }
    }
    if ($ctx.ContainsKey('WarmWindow')) {
        $window = $ctx.WarmWindow
        $processId = [VelocityDesktop.Native]::Pid($window)
        Invoke-Probe 'Forward file:3:2 to the only GUI and verify the caret' {
            $env:VELOCITY_DATA_DIR = Join-Path $ctx.CaseRoot 'decoy-profile'
            try { $null = Complete-Native (Start-Helper 'line-and-column' @('--data-dir', $ctx.Profile, "${location}:3:2")) }
            finally { $env:VELOCITY_DATA_DIR = $ctx.Profile }
            $null = Wait-Title $window 'line location.txt'
            $gui = @(Get-SuiteProcesses -GuiOnly)
            Assert-That ($gui.Count -eq 1 -and $gui[0].Id -eq $processId -and @(Get-EditorWindows).Count -eq 1) 'Forwarding created another process or window'
            Focus-Window $window
            Send-Ascii $window X
            Send-Key $window @('Ctrl', 'S')
            Expect-Bytes $location "line one`r`nline two`r`naXbcde"
            Save-Evidence $window 'forward-line-column'
        }
        Invoke-Probe '--wait blocks until Ctrl+W closes its sheet' {
            $child = Start-Helper 'wait-helper' @('--wait', $wait)
            $null = Wait-Title $window 'wait.txt'
            Assert-That (-not $child.Process.WaitForExit(350)) 'Wait helper exited while its sheet was open'
            Focus-Window $window
            Save-Evidence $window 'wait-open'
            Send-Key $window @('Ctrl', 'W')
            $null = Complete-Native $child
            $null = Wait-Until 'wait sheet no longer active' { [VelocityDesktop.Native]::Title($window) -notlike '*wait.txt*' }
            Save-Evidence $window 'wait-closed'
        }
        Invoke-Probe '-n opens a second HWND in the existing process' {
            # vel treats piped stdin as an implied wait when invoked without paths.
            $null = Complete-Native (Start-CmdHelper 'new-window' '-n < NUL')
            $extra = Wait-Until 'second editor HWND' { Get-EditorWindows $processId | Where-Object { $_ -ne $window } | Select-Object -First 1 }
            Assert-That (@(Get-SuiteProcesses -GuiOnly).Count -eq 1) 'vel -n launched another GUI process'
            Focus-Window $extra
            $null = Wait-Frame $extra
            Save-Evidence $extra 'cli-new-window'
            Send-Key $extra @('Alt', 'F4')
            $null = Wait-Until 'extra CLI window closes, leaving the original HWND' {
                $windows = @(Get-EditorWindows $processId)
                $windows.Count -eq 1 -and $windows[0] -eq $window
            }
        }
        Invoke-Probe 'PowerShell stdin becomes an editable tab and returns stdout on close' {
            Focus-Window $window
            $before = [VelocityDesktop.Native]::Title($window)
            $child = Start-Native $powershell 'stdin' @('-NoProfile', '-NonInteractive', '-Command', "'hello' | & $cliLiteral -; exit `$LASTEXITCODE")
            $null = Wait-Until 'stdin activates a new tab' { [VelocityDesktop.Native]::Title($window) -cne $before }
            Assert-That (-not $child.Process.HasExited) 'Stdin did not wait for its tab'
            Assert-That ((Read-Editor $window).TrimEnd([char[]]"`r`n") -ceq 'hello') 'Stdin did not put hello in the editor'
            Send-Text $window 'edited hello'
            $null = Wait-Until 'edited stdin tab is dirty' { [VelocityDesktop.Native]::Title($window).EndsWith([string][char]0x2022) }
            Save-Evidence $window 'stdin-edited'
            Send-Key $window @('Ctrl', 'W')
            $close = Wait-Until 'stdin closes or shows a save prompt' {
                $dialog = Get-OwnedDialog $window '^Velocity$'
                if ($dialog) { [pscustomobject]@{ Dialog=$dialog } }
                elseif ($child.Process.HasExited) { [pscustomobject]@{ Dialog=[IntPtr]::Zero } }
            }
            if ($close.Dialog -ne [IntPtr]::Zero) {
                Save-Evidence $window 'stdin-save-prompt' @($close.Dialog)
                Assert-That (-not $child.Process.HasExited) 'Stdin helper exited before the save decision'
                # This skips writing a file. The CLI still returns the edited sheet on close.
                Confirm-StdinClose $window $close.Dialog
            }
            $output = Complete-Native $child
            Assert-That ($output.TrimEnd([char[]]"`r`n") -ceq 'edited hello') 'Console stdout was not the edited stdin text'
            $null = Wait-Until 'stdin sheet closes and the preceding sheet regains its title' { [VelocityDesktop.Native]::Title($window) -ceq $before }
            Save-Evidence $window 'stdin-closed'
        }
        Invoke-Probe 'Exit before cold start' { Exit-Editor $window }
        $ctx.Remove('WarmWindow')
    }
    Invoke-Probe 'Cold vel.exe starts the adjacent GUI and forwards the file' {
        # Keep the warm exit assertion separate. Even if it fails, the cold
        # start/stream check must run from a verified process-free state.
        $stale = @(Get-SuiteProcesses -GuiOnly)
        if ($stale.Count) {
            $ctx.Check.metrics['cold_start_cleanup_pids'] = @($stale | ForEach-Object { $_.Id })
            foreach ($process in $stale) {
                if (-not $process.HasExited) { $process.Kill() }
                Assert-That ($process.WaitForExit(3000)) "Could not clean up test GUI PID $($process.Id) for cold start"
            }
        }
        Assert-That (@(Get-SuiteProcesses -GuiOnly).Count -eq 0) 'Cold start requires a process-free desktop for the packaged executable'
        $profile = Join-Path $ctx.CaseRoot 'cold-profile'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $null = Complete-Native (Start-Helper 'cold-helper' @('--data-dir', $profile, '--width', '850', '--height', '550', '--output',
                    (Join-Path $ctx.Logs 'cold-gui.jsonl'), $cold)) 30
        $window = Wait-Until 'cold-started GUI with the requested file, after both helper streams closed' {
            Get-EditorWindows | Where-Object { [VelocityDesktop.Native]::Title($_).Contains('cold.txt') } | Select-Object -First 1
        }
        Focus-Window $window
        $null = Wait-Frame $window
        $ctx.Check.metrics['cold_open_ms'] = $timer.ElapsedMilliseconds
        Save-Evidence $window 'cold-opened'
        Exit-Editor $window
    }
}

function New-EditorWindow([IntPtr] $From) {
    $processId = [VelocityDesktop.Native]::Pid($From)
    $old = @(Get-EditorWindows $processId)
    Focus-Window $From
    Send-Key $From @('Ctrl', 'Shift', 'N')
    $new = Wait-Until 'Ctrl+Shift+N creates another window' { Get-EditorWindows $processId | Where-Object { $_ -notin $old } | Select-Object -First 1 }
    Focus-Window $new
    $null = Wait-Frame $new
    $new
}

function Wait-SessionText([string] $Text) {
    $path = Join-Path $ctx.Profile 'Sessions/Session.sublime-session'
    $null = Wait-Until 'dirty text is in the durable session generation' {
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            try { (Read-SharedText $path).Contains($Text) } catch [IO.IOException] { $false }
        }
    } 15
    $path
}

function Test-Session {
    $a = Join-Path $ctx.CaseRoot 'first-session.txt'
    $b = Join-Path $ctx.CaseRoot 'second-session.txt'
    Write-Utf8 $a 'first'
    Write-Utf8 $b 'second'
    $first = Start-Editor 'before-exit' @($a)
    $markerA = 'unsaved first window'
    $markerB = 'unsaved second window'
    Send-Key $first @('Ctrl', 'A')
    Send-Text $first $markerA
    $null = Wait-SessionText $markerA
    Save-Evidence $first 'first-dirty'
    $second = New-EditorWindow $first
    Open-File $second $b 'open-in-second'
    Send-Key $second @('Ctrl', 'A')
    Send-Text $second $markerB
    $null = Wait-SessionText $markerB
    $from = [VelocityDesktop.Native]::Outer($second)
    $desktop = [VelocityDesktop.Native]::DesktopBounds()
    [VelocityDesktop.Native]::MoveResize($second, $desktop.X + 54, $desktop.Y + 52,
        [Math]::Min(720, $desktop.Width - 105), [Math]::Min(510, $desktop.Height - 105))
    $moved = Wait-Until 'changed position and size' {
        $actual = [VelocityDesktop.Native]::Outer($second)
        if ($actual.Location -ne $from.Location -and $actual.Size -ne $from.Size) { $actual }
    }
    $null = Wait-Frame $second
    Save-Evidence $second 'second-moved-resized'
    $third = New-EditorWindow $second
    Save-Evidence $third 'third-window'
    Send-Key $third @('Alt', 'F4')
    $null = Wait-Until 'Alt+F4 closed just the third window' { @(Get-EditorWindows).Count -eq 2 -and $third -notin @(Get-EditorWindows) }
    Focus-Window $second
    Save-Evidence $second 'after-alt-f4'
    Exit-Editor $second
    Assert-That (@(Get-SuiteProcesses).Count -eq 0) 'A packaged process survived File > Exit'
    Save-Evidence ([IntPtr]::Zero) 'after-file-exit'

    $null = Start-Editor 'restored'
    $restored = @(Wait-Until 'two windows restored' {
        $windows = @(Get-EditorWindows)
        if ($windows.Count -eq 2) { $windows }
    })
    for ($i = 0; $i -lt 2; $i++) {
        $name = @('first-session.txt', 'second-session.txt')[$i]
        $expected = @($markerA, $markerB)[$i]
        $window = $restored | Where-Object { [VelocityDesktop.Native]::Title($_).Contains($name) } | Select-Object -First 1
        Assert-That ($null -ne $window) "No restored title for $name"
        Focus-Window $window
        $null = Wait-Frame $window
        Assert-That ((Read-Editor $window) -ceq $expected) "Restored buffer $name lost unsaved text"
        Save-Evidence $window "restored-$($i+1)"
        if ($i -eq 1) {
            $bounds = [VelocityDesktop.Native]::Outer($window)
            $ctx.Check.metrics['window_before_exit'] = [string]$moved
            $ctx.Check.metrics['window_after_restore'] = [string]$bounds
            foreach ($dimension in 'X','Y','Width','Height') {
                Assert-That ([Math]::Abs($bounds.$dimension - $moved.$dimension) -le 16) "Restored bounds differ in $dimension"
            }
        }
    }
    Expect-Bytes $a 'first'
    Expect-Bytes $b 'second'
    Exit-Editor $window
    Assert-That (@(Get-SuiteProcesses).Count -eq 0) 'Orphaned packaged process after final Exit'
}

function Test-Project {
    $folder = Join-Path $ctx.CaseRoot 'sample project'
    [IO.Directory]::CreateDirectory($folder) | Out-Null
    $doc = Join-Path $folder 'keyboard-open.txt'
    Write-Utf8 $doc 'opened from the sidebar'
    $window = Start-Editor 'project'
    $before = Get-Frame $window left
    try {
        $null = Complete-Native (Start-Helper 'add-folder' @($folder))
        $null = Wait-Title $window 'sample project'
        $null = Wait-Changed $window $before left
        Save-Evidence $window 'folder-added'
    } finally { $before.Dispose() }
    Focus-Window $window
    Send-Key $window @('Ctrl', '0')
    Send-Key $window Up
    # A newly added root is normally open; use Right to select its child or expand it.
    $null = Wait-Until 'keyboard opens the sole sidebar file' {
        if ([VelocityDesktop.Native]::Title($window).Contains('keyboard-open.txt')) { return $true }
        # A new root is expanded by the product; Right selects the first loaded leaf.
        Send-Key $window Right
        Send-Key $window Enter
    } 8
    Assert-That ((Read-Editor $window) -ceq 'opened from the sidebar') 'Sidebar opened the wrong source'
    Save-Evidence $window 'keyboard-opened'
}

function Test-Settings {
    $window = Start-Editor 'settings'
    Invoke-Probe 'Preferences > Settings opens a native window with both JSON panes' {
        Invoke-Menu $window N '^Settings\u2026$' 'preferences-json-menu'
        $pair = Wait-Until 'paired settings window appears' {
            Get-EditorWindows | Where-Object { $_ -ne $window -and [VelocityDesktop.Native]::Title($_) -eq 'Preferences.sublime-settings' } | Select-Object -First 1
        }
        Focus-Window $pair
        $null = Wait-Frame $pair
        $right = Read-Editor $pair
        Assert-That ($right -match '"ui_language"\s*:\s*"en"') 'Right pane did not show User Preferences'
        Send-Key $pair @('Ctrl', '1')
        $left = Read-Editor $pair
        Assert-That ($left -match '"default_line_ending"') 'Left pane did not show bundled Default Preferences'
        Send-Key $pair @('Ctrl', '2')
        Save-Evidence $pair 'paired-json'
        Send-Key $pair @('Alt', 'F4')
        $null = Wait-Until 'paired settings window closed' { $pair -notin @(Get-EditorWindows) }
    }
    Invoke-Probe 'Preferences > Settings (Graphical) writes User Preferences' {
        Focus-Window $window
        $editor = Get-Frame $window
        try {
            Invoke-Menu $window N '^Settings \(Graphical\)\u2026$' 'preferences-graphical-menu'
            $null = Wait-Until 'graphical Settings title' { [VelocityDesktop.Native]::Title($window) -eq 'Settings' }
            $null = Wait-Changed $window $editor client
        } finally { $editor.Dispose() }
        Save-Evidence $window 'graphical'
        $before = Get-Frame $window top
        try { Send-Text $window 'line_numbers'; $null = Wait-Changed $window $before top }
        finally { $before.Dispose() }
        Send-Key $window Enter # Search -> results
        Send-Key $window Enter # First result -> toggle
        $null = Wait-Until 'line_numbers is false in User Preferences' {
            try {
                $preferences = ConvertFrom-Json (Read-SharedText $ctx.Preferences)
                $setting = $preferences.PSObject.Properties['line_numbers']
                $null -ne $setting -and $setting.Value -is [bool] -and -not $setting.Value
            } catch [IO.IOException] { $false }
        }
        Copy-Item -LiteralPath $ctx.Preferences -Destination (Join-Path $ctx.Logs 'User-Preferences.json')
        Save-Evidence $window 'line-numbers-off'
    }
}

function Test-LargeFile {
    Set-Preferences @{ word_wrap=$false }
    $file = Join-Path $ctx.CaseRoot 'large.txt'
    $writer = [IO.StreamWriter]::new($file, $false, [Text.UTF8Encoding]::new($false), 65536)
    try {
        $template = 'ROW {0:D7} Velocity Windows desktop test data, long enough to check scrolling.'
        $rows = [int]((50MB) / (($template -f 0).Length + 2))
        for ($i = 0; $i -lt $rows; $i++) { $writer.WriteLine(($template -f $i)) }
    } finally { $writer.Dispose() }
    $window = Start-Editor 'large-fixture'
    $before = Get-Frame $window text
    try {
        $timer = [Diagnostics.Stopwatch]::StartNew()
        Open-File $window $file 'open-50-mib' 30
        $changed = Wait-Changed $window $before text 2500
        $ctx.Check.metrics['file_open'] = @{ file_bytes=(Get-Item -LiteralPath $file).Length; first_content_frame_ms=$timer.ElapsedMilliseconds;
            changed_pixels=$changed; working_set_bytes=(Get-Process -Id ([VelocityDesktop.Native]::Pid($window))).WorkingSet64 }
    } finally { $before.Dispose() }
    $row = Get-LargeRow $window
    Assert-That ($row -eq 0) "Unexpected first visible line of the large document: $row"
    Save-Evidence $window 'first-content-frame'
    for ($page = 1; $page -le 3; $page++) {
        $before = Get-Frame $window text
        try { Send-Key $window PageDown; $null = Wait-Changed $window $before text 180 }
        finally { $before.Dispose() }
        $next = Get-LargeRow $window
        Assert-That ($next -gt $row) "PageDown did not move down in the file (row $row to $next)"
        $ctx.Check.metrics["page_${page}_row"] = $next
        $row = $next
        Save-Evidence $window "page-down-$page"
    }
}

function Get-LargeRow([IntPtr] $Window) {
    Send-Key $Window Home
    Send-Key $Window @('Shift', 'End')
    $line = Read-Editor $Window -SelectionOnly
    Assert-That ($line -match '^ROW (\d{7}) ') "The large-file selection is not a fixture line: $line"
    $number = [int]$Matches[1]
    Send-Key $Window Home
    $number
}

function Read-RegistryValue([string] $Key, [string] $Name) {
    if (Test-Path -LiteralPath $Key) { (Get-Item -LiteralPath $Key).GetValue($Name) }
}

function Read-UninstallRegistration([string] $Key, [Microsoft.Win32.RegistryView] $View) {
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('CurrentUser', $View)
    try {
        $entry = $base.OpenSubKey($Key)
        if ($null -eq $entry) { return }
        try {
            [ordered]@{ display_name=$entry.GetValue('DisplayName'); version=$entry.GetValue('DisplayVersion');
                command=$entry.GetValue('UninstallString'); view=$View.ToString() }
        } finally { $entry.Dispose() }
    } finally { $base.Dispose() }
}

function Get-PathEntries([string] $Value) {
    $Value -split ';' | ForEach-Object { $_.Trim().TrimEnd('\') } | Where-Object { $_ }
}

function Test-Installer {
    Assert-That ($null -ne $ctx.Setup) "Missing validated setup.exe: $($ctx.PackageError)"
    $dir = Join-Path $env:LOCALAPPDATA 'Programs/Velocity'
    $shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Velocity.lnk'
    $uninstall = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.velocity.Velocity_is1'
    $classes = 'HKCU:\Software\Classes'
    $ownerKey = 'HKCU:\Software\Velocity\Installer'
    $registrations = @("$classes\Velocity.TextFile", "$classes\Applications\velocity.exe")
    $extensions = 'txt','text','log','md','json','toml','yaml','yml','ini','csv','xml'
    $originalPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $originalChoice = Read-RegistryValue 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.txt\UserChoice' 'ProgId'
    foreach ($key in @($dir, $shortcut, $ownerKey) + $registrations) {
        Assert-That (-not (Test-Path -LiteralPath $key)) "Existing Velocity installation/state; refusing to change $key"
    }
    foreach ($view in 'Registry64','Registry32') {
        Assert-That ($null -eq (Read-UninstallRegistration $uninstall $view)) "Existing per-user uninstall registration ($view)"
    }
    Assert-That ($dir -notin @(Get-PathEntries $originalPath)) 'Velocity is already present in the user PATH'
    foreach ($extension in $extensions) {
        Assert-That ($null -eq (Read-RegistryValue "$classes\.$extension\OpenWithProgids" 'Velocity.TextFile')) "Existing .$extension association"
    }
    $ctx.InstalledGui = Join-Path $dir 'velocity.exe'
    $ctx.InstalledCli = Join-Path $dir 'vel.exe'
    $ctx.Check.metrics['smartscreen'] = 'Not exercised: silent installer on a disposable runner'
    try {
        $null = Complete-Native (Start-Native $ctx.Setup 'install' @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-',
                    '/TASKS=addpath', ('/LOG=' + (Join-Path $ctx.Logs 'setup.log')))) 75
        Invoke-Probe 'Start menu and per-user uninstall registration' {
            $link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
            Assert-That ((Test-Path -LiteralPath $shortcut) -and $link.TargetPath -ieq $ctx.InstalledGui) 'Missing or incorrect Start menu shortcut'
            $entry = Wait-Until 'per-user uninstall registry entry (64-bit view)' { Read-UninstallRegistration $uninstall Registry64 }
            $ctx.Check.metrics['uninstall_registration'] = $entry
            # Inno's default DisplayName is AppVerName, not the bare AppName.
            Assert-That ($entry.display_name -match '^Velocity(\s|$)') "Wrong uninstall display name: $($entry.display_name)"
            Assert-That ($entry.version -eq $report.package.version) "Wrong uninstall version: $($entry.version)"
            $uninstallExe = $entry.command -replace '^"([^"]+)".*$', '$1'
            Assert-That ((Split-Path -Parent $uninstallExe) -ieq $dir -and (Test-Path -LiteralPath $uninstallExe -PathType Leaf)) 'UninstallString does not point to the installed uninstaller'
        }
        Invoke-Probe 'Explorer Open with registrations, without replacing UserChoice' {
            foreach ($registration in $registrations) {
                Assert-That ((Read-RegistryValue "$registration\shell\open\command" '') -eq ('"' + $ctx.InstalledGui + '" "%1"')) "Invalid Open with command: $registration"
            }
            foreach ($extension in $extensions) {
                Assert-That ($null -ne (Read-RegistryValue "$classes\.$extension\OpenWithProgids" 'Velocity.TextFile')) "Missing .$extension Open with registration"
            }
            Assert-That ((Read-RegistryValue 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.txt\UserChoice' 'ProgId') -eq $originalChoice) 'Installer changed the default .txt handler'
        }
        Invoke-Probe 'PATH resolves the installed vel.exe' {
            $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
            Assert-That ($dir -in @(Get-PathEntries $userPath)) 'User PATH does not include the install directory'
            $old = $env:Path
            try {
                # The current host retains its startup environment; resolve using the registered user PATH.
                $env:Path = $userPath + ';' + $old
                $helper = (Get-Command vel.exe -CommandType Application).Source
                Assert-That ($helper -ieq $ctx.InstalledCli) "PATH resolves $helper instead of the installed vel.exe"
                $out = Complete-Native (Start-Native $helper 'installed-helper' @('--version'))
                Assert-That ($out.Trim() -eq ('Velocity ' + $report.package.version)) 'Unexpected installed helper version'
            } finally { $env:Path = $old }
        }
        Invoke-Probe 'Installed executable renders on the desktop' {
            $window = Start-Editor 'installed' -Executable $ctx.InstalledGui
            Save-Evidence $window 'installed-window'
            Exit-Editor $window
        }
    } finally {
        # Do not strand a silent install when a GUI or registration assertion fails.
        foreach ($process in @(Get-SuiteProcesses)) { $process.Kill(); $null = $process.WaitForExit(3000) }
        $uninstaller = Get-ChildItem -LiteralPath $dir -Filter 'unins*.exe' -File -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($uninstaller) {
            $null = Complete-Native (Start-Native $uninstaller.FullName 'uninstall' @('/VERYSILENT', '/SUPPRESSMSGBOXES',
                        '/NORESTART', ('/LOG=' + (Join-Path $ctx.Logs 'uninstall.log')))) 75
        }
        Invoke-Probe 'Uninstall removes binaries, shortcut, Open with and PATH entry' {
            $null = Wait-Until 'uninstaller removes packaged executables' { -not (Test-Path -LiteralPath $ctx.InstalledGui) -and -not (Test-Path -LiteralPath $ctx.InstalledCli) }
            foreach ($key in @($shortcut, $ownerKey) + $registrations) {
                Assert-That (-not (Test-Path -LiteralPath $key)) "Uninstall left $key"
            }
            foreach ($view in 'Registry64','Registry32') {
                Assert-That ($null -eq (Read-UninstallRegistration $uninstall $view)) "Uninstall registration remains ($view)"
            }
            foreach ($extension in $extensions) {
                Assert-That ($null -eq (Read-RegistryValue "$classes\.$extension\OpenWithProgids" 'Velocity.TextFile')) "Uninstall left the .$extension association"
            }
            Assert-That ([string][Environment]::GetEnvironmentVariable('Path','User') -ceq [string]$originalPath) 'Uninstall did not preserve the original user PATH'
            Assert-That (-not (Test-Path -LiteralPath (Join-Path $dir 'Resources'))) 'Uninstall left packaged resources'
            if (-not $ctx.DesktopError) { Save-Evidence ([IntPtr]::Zero) 'after-uninstall' }
        }
    }
}

function Test-Recovery {
    $file = Join-Path $ctx.CaseRoot 'recover.txt'
    $marker = 'dirty text to recover after termination'
    Write-Utf8 $file 'unchanged on disk'
    $window = Start-Editor 'before-kill' @($file)
    Send-Key $window @('Ctrl', 'A')
    Send-Text $window $marker
    $path = Wait-SessionText $marker
    Copy-Item -LiteralPath $path -Destination (Join-Path $ctx.Logs 'pre-kill.sublime-session')
    Save-Evidence $window 'dirty-durable'
    $processId = [VelocityDesktop.Native]::Pid($window)
    $ctx.Check.metrics['forced_pid'] = $processId
    Stop-Process -Id $processId -Force
    $null = Wait-Until 'terminated process and HWND gone' { @(Get-SuiteProcesses).Count -eq 0 -and @(Get-EditorWindows).Count -eq 0 }
    Expect-Bytes $file 'unchanged on disk'
    $restored = Start-Editor 'after-kill'
    $null = Wait-Title $restored 'recover.txt'
    Assert-That ((Read-Editor $restored) -ceq $marker) 'Dirty autosaved text was not recovered'
    Save-Evidence $restored 'restored-dirty-buffer'
    Exit-Editor $restored
}

function Test-Pinyin {
    if (-not (Get-Command Get-WinUserLanguageList -ErrorAction SilentlyContinue)) {
        $ctx.Check.skip_reason = 'The runner does not have the inbox user language-list cmdlet.'
        return
    }
    $original = Get-WinUserLanguageList
    $pinyin = '0804:{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}{FA550B04-5AD7-411F-A5AC-CA038EC515D7}'
    $tips = @($original | ForEach-Object { $_.InputMethodTips })
    $ctx.Check.metrics['original_input_method_tips'] = $tips
    $restore = $false
    try {
        if ($pinyin -notin $tips -or @($tips | Where-Object { $_ -like '0804:*' }).Count -ne 1) {
            $ime = Join-Path $env:windir 'System32/InputMethod/CHS/ChsIME.exe'
            if ($env:GITHUB_ACTIONS -ne 'true' -or -not (Test-Path -LiteralPath $ime -PathType Leaf)) {
                $ctx.Check.skip_reason = 'Pinyin is not configured and no inbox Pinyin activation will be attempted on this machine.'
                return
            }
            $restore = $true
            try {
                $list = Get-WinUserLanguageList
                $zh = $list | Where-Object { $_.LanguageTag -eq 'zh-CN' } | Select-Object -First 1
                if (-not $zh) {
                    $zh = (New-WinUserLanguageList -Language 'zh-CN')[0]
                    $list.Add($zh)
                }
                $zh.InputMethodTips.Clear()
                $zh.InputMethodTips.Add($pinyin)
                Set-WinUserLanguageList -LanguageList $list -Force
                $tips = @(Get-WinUserLanguageList | ForEach-Object { $_.InputMethodTips })
                if ($pinyin -notin $tips -or @($tips | Where-Object { $_ -like '0804:*' }).Count -ne 1) {
                    $ctx.Check.skip_reason = 'The inbox language setting did not register a unique Pinyin input method.'
                    return
                }
            } catch {
                $ctx.Check.skip_reason = 'Could not enable inbox Pinyin without installing capabilities: ' + $_.Exception.Message
                return
            }
        }
        $ctx.Check.metrics['input_method_tips'] = $tips
        Test-PinyinComposition $tips.Count
    } finally {
        if ($restore) { Set-WinUserLanguageList -LanguageList $original -Force }
    }
}

function Test-PinyinComposition([int] $TipCount) {
    $window = Start-Editor 'pinyin'
    for ($i = 0; $i -lt $TipCount -and [VelocityDesktop.Native]::InputLanguage($window) -ne 0x0804; $i++) {
        Send-Key $window @('Win', 'Space')
        try { $null = Wait-Until 'Pinyin active in editor' { [VelocityDesktop.Native]::InputLanguage($window) -eq 0x0804 } 2 }
        catch { Focus-Window $window }
    }
    if ([VelocityDesktop.Native]::InputLanguage($window) -ne 0x0804) {
        $ctx.Check.skip_reason = 'Configured Pinyin could not be selected in the runner desktop without changing its configuration.'
        return
    }
    Focus-Window $window
    $old = @([VelocityDesktop.Native]::Windows(0))
    Send-Ascii $window nihao
    $candidate = Wait-Until 'IME candidate HWND' {
        [VelocityDesktop.Native]::Windows(0) | Where-Object { $_ -notin $old -and
            [VelocityDesktop.Native]::Class($_) -match 'Candidate|IME.*UI' } | Select-Object -First 1
    } 5
    $ctx.Check.metrics['candidate_rect'] = [string][VelocityDesktop.Native]::Outer($candidate)
    Save-Evidence $window 'candidate-position' @($candidate)
    Send-Key $window Space
    $null = Wait-Until 'IME candidate window dismissed' { $candidate -notin [VelocityDesktop.Native]::Windows(0) }
    Assert-That ((Read-Editor $window) -match '[\u4e00-\u9fff]') 'Pinyin committed no Han characters'
    Save-Evidence $window 'committed'
}
