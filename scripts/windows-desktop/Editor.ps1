# Editor-side checks. The virtual editor is not yet a UIA provider; prefer byte,
# clipboard and HWND assertions, and use pixel comparisons for virtual panels.

function Test-Launch {
    $window = Start-Editor 'portable'
    Save-Evidence $window 'first-frame'
    Assert-That ($ctx.Check.metrics.portable.awareness -eq 'PerMonitorV2') 'Expected a per-monitor v2-aware window'
    Assert-That ($ctx.Check.metrics.portable.dpi -gt 0) 'HWND returned no DPI'
    $adapter = (Read-SharedText (Join-Path $ctx.Logs 'portable.jsonl')) -split "`n" |
        Where-Object { $_ -match '"kind":"dx12_adapter"' } | Select-Object -First 1
    Assert-That ([bool]$adapter) 'Missing DX12 adapter selection in startup log'
    $ctx.Check.metrics['adapter'] = ConvertFrom-Json $adapter
}

function Measure-Ink([Drawing.Rectangle] $Area, [int] $Expected) {
    $image = [VelocityDesktop.Native]::Screen($Area)
    try { $colors = [VelocityDesktop.Native]::Histogram($image) } finally { $image.Dispose() }
    $top = @($colors.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 8)
    $ink = 0
    foreach ($color in $colors.GetEnumerator()) {
        if ([Math]::Abs((($color.Key -shr 16) -band 255) - (($Expected -shr 16) -band 255)) -le 12 -and
            [Math]::Abs((($color.Key -shr 8) -band 255) - (($Expected -shr 8) -band 255)) -le 12 -and
            [Math]::Abs(($color.Key -band 255) - ($Expected -band 255)) -le 12) { $ink += $color.Value }
    }
    [ordered]@{ expected=('#{0:X6}' -f $Expected); near_expected_pixels=$ink;
        top_colors=@($top | ForEach-Object { '#{0:X6}: {1}' -f $_.Key, $_.Value }) }
}

function Test-Rendering {
    Set-Preferences @{ font_face='Consolas'; font_size=18; gutter=$false; line_numbers=$false;
        show_minimap=$false; draw_indent_guides=$false; highlight_line=$false; caret_style='solid';
        color_scheme='Velocity Dark.sublime-color-scheme' }
    $sample = Join-Path $ctx.CaseRoot 'syntax.py'
    $source = @('def first():', '    return                          plain_identifier', '',
        'def second():', '    return                          plain_identifier', '',
        'def third():', '    return                          plain_identifier', '', ('# ' + $fixture.unicode)) -join "`r`n"
    Write-Utf8 $sample ($source + "`r`n")
    $window = Start-Editor 'syntax' @($sample)
    $null = Wait-Title $window 'syntax.py'
    $null = Wait-Until 'distinct keyword and identifier ink' {
        $area = [VelocityDesktop.Native]::Client($window)
        $scale = [VelocityDesktop.Native]::GetDpiForWindow($window) / 96.0
        # The bundled dark scheme uses violet for keywords and fg for identifiers.
        $keyword = Measure-Ink ([Drawing.Rectangle]::new(($area.X + 18*$scale), ($area.Y + 59*$scale), (145*$scale), (142*$scale))) 0xc9acf1
        $identifier = Measure-Ink ([Drawing.Rectangle]::new(($area.X + 300*$scale), ($area.Y + 59*$scale), (270*$scale), (142*$scale))) 0xe3ebee
        $ctx.Check.metrics['ink'] = [ordered]@{ keyword=$keyword; identifier=$identifier }
        if ($keyword.near_expected_pixels -ge 20 -and $identifier.near_expected_pixels -ge 20) { $true }
    } 15
    Save-Evidence $window 'highlighted-python'
}

function Test-Typing {
    $file = Join-Path $ctx.CaseRoot 'typing.txt'
    Write-Utf8 $file "seed`r`n"
    $window = Start-Editor 'typing' @($file)
    Send-Key $window @('Ctrl', 'A')
    Send-Ascii $window alpha
    Send-Key $window Enter
    Send-Text $window ($fixture.unicode + 'x')
    Send-Key $window Home
    Send-Key $window End
    Send-Key $window Backspace
    Send-Key $window Up
    Send-Key $window Home
    Send-Key $window Right
    Send-Ascii $window R
    Send-Key $window Left
    Send-Ascii $window L
    Send-Key $window Down
    Send-Key $window End
    Send-Key $window @('Ctrl', 'S')
    $baseline = "aLRlpha`r`n" + $fixture.unicode
    Expect-Bytes $file $baseline
    Save-Evidence $window 'typed-and-saved'
    Send-Ascii $window Z
    Send-Key $window @('Ctrl', 'S')
    Expect-Bytes $file ($baseline + 'Z')
    Save-Evidence $window 'before-undo'
    Send-Key $window @('Ctrl', 'Z')
    Send-Key $window @('Ctrl', 'S')
    Expect-Bytes $file $baseline
    Save-Evidence $window 'undo'
    Send-Key $window @('Ctrl', 'Y')
    Send-Key $window @('Ctrl', 'S')
    Expect-Bytes $file ($baseline + 'Z')
    Save-Evidence $window 'redo'
}

function Save-ThroughDialog([IntPtr] $Window, [string] $Path) {
    $processId = [VelocityDesktop.Native]::Pid($Window)
    Send-Key $Window @('Ctrl', 'S')
    $dialog = Wait-Until 'native Save As dialog' {
        foreach ($candidate in [VelocityDesktop.Native]::Windows($processId)) {
            if ([VelocityDesktop.Native]::Class($candidate) -ne '#32770') { continue }
            $root = [System.Windows.Automation.AutomationElement]::FromHandle($candidate)
            if ($root.Current.Name -match '^Save As:?$') { $candidate; break }
        }
    }
    Focus-Window $dialog
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($dialog)
    $edit = Wait-Until 'Save As file name edit through UI Automation' {
        $controls = @(Get-Elements $root Edit)
        $controls | Where-Object {
            $label = $_.Current.LabeledBy
            $_.Current.AutomationId -eq '1001' -or $_.Current.HasKeyboardFocus -or
                ($null -ne $label -and $label.Current.Name -match 'File name')
        } | Select-Object -First 1
    }
    $edit.SetFocus()
    Send-Key $dialog @('Ctrl', 'A')
    Send-Text $dialog $Path
    $value = $edit.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
    $null = Wait-Until 'Unicode file name visible in dialog' { $value.Current.Value -ceq $Path }
    Save-Evidence $Window 'native-dialog' @($dialog)
    $save = Get-Elements $root Button | Where-Object { $_.Current.AutomationId -eq '1' -or $_.Current.Name -eq 'Save' } | Select-Object -First 1
    Assert-That ($null -ne $save) 'UIA did not expose the Save button'
    $save.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
    $null = Wait-Until 'Save As dismissed and editor foreground' { [VelocityDesktop.Native]::Foreground -eq $Window }
    $null = Wait-Title $Window ([IO.Path]::GetFileName($Path))
}

function Test-SaveAs {
    $window = Start-Editor 'save-as'
    Send-Key $window @('Ctrl', 'N')
    Send-Text $window $fixture.cjk
    Send-Key $window Enter
    Send-Ascii $window 'second line'
    $name = Join-Path $ctx.CaseRoot $fixture.filename
    Save-ThroughDialog $window $name
    # The default setting is "system": vel-fileio uses CRLF on Windows.
    Expect-Bytes $name ($fixture.cjk + "`r`nsecond line")
    Save-Evidence $window 'saved-tab'
}

function Dismiss-Menu([IntPtr] $Window) {
    if ([VelocityDesktop.Native]::InMenu($Window)) {
        $null = Wait-Until 'menu mode closed' {
            if (-not [VelocityDesktop.Native]::InMenu($Window)) { return $true }
            Send-Key $Window Esc
        } 4
    }
}

function Read-NativeMenuTree([IntPtr] $Menu) {
    for ($i = 0; $i -lt [VelocityDesktop.Native]::GetMenuItemCount($Menu); $i++) {
        $raw = [VelocityDesktop.Native]::MenuLabel($Menu, $i)
        $parts = $raw -split "`t", 2
        $children = [VelocityDesktop.Native]::GetSubMenu($Menu, $i)
        [ordered]@{ caption=(Get-MenuCaption $raw); raw=$raw;
            shortcut=$(if ($parts.Count -eq 2) { $parts[1] } else { '' });
            children=$(if ($children -ne [IntPtr]::Zero) { ,@(Read-NativeMenuTree $children) } else { ,@() }) }
    }
}

function Read-MenuLanguage([IntPtr] $Window, [string] $Language, $Result) {
    $expected = if ($Language -eq 'en') { $fixture.enMenus } else { $fixture.zhMenus }
    $keys = 'F','E','S','I','V','G','T','P','N','H'
    Invoke-Probe "$Language Alt menu bar" {
        try {
            Send-Key $Window Alt
            $null = Wait-Until 'Alt enters native menu mode' { [VelocityDesktop.Native]::InMenu($Window) }
            Save-Evidence $Window "$Language-alt"
            $Result.top = @(Get-TopMenus $Window | ForEach-Object { Get-MenuCaption $_.Current.Name })
            Assert-That (($Result.top -join '|') -ceq ($expected -join '|')) "UIA top-level captions: $($Result.top -join ', ')"
        } finally { Dismiss-Menu $Window }
    }
    for ($i = 0; $i -lt $keys.Count; $i++) {
        $key = $keys[$i]
        $caption = $expected[$i]
        Invoke-Probe "$Language Alt+$key ($caption)" {
            try {
                Send-Key $Window @('Alt', $key)
                $popup = @(Wait-Until "UIA items for Alt+$key" { @(Get-PopupItems $Window) })
                Save-Evidence $Window "$Language-$($fixture.enMenus[$i].ToLowerInvariant())"
                $uia = @($popup | ForEach-Object { [ordered]@{ name=$_.Current.Name; access=$_.Current.AccessKey;
                    accelerator=$_.Current.AcceleratorKey; enabled=$_.Current.IsEnabled } })
                $bar = [VelocityDesktop.Native]::MenuBar($Window)
                $top = [VelocityDesktop.Native]::MenuLabel($bar, $i)
                $native = @(Read-NativeMenuTree ([VelocityDesktop.Native]::GetSubMenu($bar, $i)))
                $Result.menus[$key] = [ordered]@{ title=$top; uia=$uia; native=$native }
                Assert-That ((Get-MenuCaption $top) -ceq $caption -and $top -match "&$key") "Native mnemonic or caption differs: $top"
                $visible = @($uia | ForEach-Object { Get-MenuCaption $_.name } | Where-Object { $_ })
                $nativeLabels = @($native | ForEach-Object { $_.caption } | Where-Object { $_ })
                Assert-That ($nativeLabels.Count -gt 0 -and ($visible -ccontains $nativeLabels[0])) "Alt+$key did not show the expected menu ($($visible -join ', '))"
                $missing = @($nativeLabels | Where-Object { $visible -cnotcontains $_ })
                Assert-That (-not $missing.Count) "UIA omitted menu items: $($missing -join ', ')"
                if ($key -eq 'F') {
                    $exit = if ($Language -eq 'en') { 'Exit' } else { $fixture.zhExit }
                    $new = if ($Language -eq 'en') { 'New File' } else { $fixture.zhNewFile }
                    Assert-That (@($native | Where-Object { $_.caption -ceq $exit -and $_.shortcut -ieq 'Ctrl+Q' }).Count -eq 1) 'File > Exit did not show Ctrl+Q'
                    Assert-That (@($native | Where-Object { $_.caption -ceq $new -and $_.shortcut -ieq 'Ctrl+N' }).Count -eq 1) 'File > New File did not show Ctrl+N'
                }
                if ($key -eq 'H') {
                    $about = if ($Language -eq 'en') { 'About Velocity' } else { $fixture.zhAbout }
                    Assert-That ($visible -ccontains $about) 'Help does not contain the localized About item'
                }
                foreach ($item in $native) {
                    Assert-That ($item.shortcut -notmatch '[\u2318\u2325\u2303\u21E7]') "Mac shortcut glyph in Windows menu: $($item.raw)"
                }
            } finally { Dismiss-Menu $Window }
        }
    }
}

function Find-Untranslated($English, $Chinese, [string] $Parent) {
    foreach ($item in $Chinese) {
        if ($item.caption -match '[A-Za-z]{2,}' -and $item.caption -cmatch '^[\x20-\x7E\u2013\u2026]+$') {
            if (@($English | ForEach-Object { $_.caption }) -ccontains $item.caption) { "$Parent > $($item.caption)" }
        }
        $index = [array]::IndexOf($Chinese, $item)
        if ($item.children.Count -and $English.Count -eq $Chinese.Count) {
            Find-Untranslated $English[$index].children $item.children "$Parent > $($item.caption)"
        }
    }
}

function Test-Menus {
    $english = [ordered]@{ top=@(); menus=[ordered]@{} }
    $chinese = [ordered]@{ top=@(); menus=[ordered]@{} }
    Invoke-Probe 'English menu pass' {
        $window = Start-Editor 'menus-en'
        Read-MenuLanguage $window 'en' $english
        Exit-Editor $window
    }
    if (@(Get-SuiteProcesses -GuiOnly).Count) {
        foreach ($process in @(Get-SuiteProcesses -GuiOnly)) { $process.Kill(); $null = $process.WaitForExit(3000) }
    }
    Set-Preferences @{ ui_language='zh-Hans' }
    Invoke-Probe 'Simplified Chinese restart' {
        $window = Start-Editor 'menus-zh'
        Read-MenuLanguage $window 'zh-Hans' $chinese
    }
    $leftovers = @(foreach ($key in $english.menus.Keys) {
        if ($chinese.menus.Contains($key)) { Find-Untranslated $english.menus[$key].native $chinese.menus[$key].native "Alt+$key" }
    })
    $ctx.Check.metrics['unchanged_english_captions'] = $leftovers
    if ($leftovers.Count) { Write-Host "Review $($leftovers.Count) unchanged English captions (some may be names or encoding labels)." }
    Write-Json (Join-Path $ctx.Logs 'menus.json') @{ english=$english; chinese=$chinese; unchanged_english_captions=$leftovers }
}

function Test-Overlay([IntPtr] $Window, [string] $Step, [string[]] $Keys, [string] $Region, [string] $Query = '') {
    $before = Get-Frame $Window $Region
    try {
        Send-Key $Window $Keys
        $null = Wait-Changed $Window $before $Region
        if ($Query) {
            $open = Get-Frame $Window $Region
            try { Send-Text $Window $Query; $null = Wait-Changed $Window $open $Region 50 }
            finally { $open.Dispose() }
        }
        Save-Evidence $Window $Step
        Send-Key $Window Esc
        $null = Wait-Until "close $Step overlay" {
            $now = Get-Frame $Window $Region
            try { $before.Size -eq $now.Size -and [VelocityDesktop.Native]::Difference($before, $now) -lt 400 }
            finally { $now.Dispose() }
        }
    } finally { $before.Dispose() }
}

function Test-Shortcuts {
    $a = Join-Path $ctx.CaseRoot 'shortcut-a.txt'
    $b = Join-Path $ctx.CaseRoot 'shortcut-b.txt'
    $rust = Join-Path $ctx.CaseRoot 'comment.rs'
    Write-Utf8 $a "kiwi kiwi`r`nsecond`r`nthird"
    Write-Utf8 $b 'second tab'
    Write-Utf8 $rust 'fn main() {}'
    $window = Start-Editor 'shortcuts' @($a)
    foreach ($spec in @(
        @{ name='Ctrl+Shift+P command palette'; step='palette'; keys=@('Ctrl','Shift','P'); region='top'; text='Toggle Side Bar' },
        @{ name='Ctrl+P Goto Anything'; step='goto-anything'; keys=@('Ctrl','P'); region='top'; text='shortcut-a' },
        @{ name='Ctrl+F Find'; step='find'; keys=@('Ctrl','F'); region='bottom'; text='' },
        @{ name='Ctrl+H Replace'; step='replace'; keys=@('Ctrl','H'); region='bottom'; text='' },
        @{ name='Ctrl+Shift+F Find in Files'; step='find-in-files'; keys=@('Ctrl','Shift','F'); region='bottom'; text='' }
    )) {
        Invoke-Probe $spec.name {
            try { Test-Overlay $window $spec.step $spec.keys $spec.region $spec.text }
            catch { Save-FailureEvidence ($spec.step + '-unexpected'); Send-Key $window Esc; throw }
        }
    }
    Invoke-Probe 'Ctrl+G Goto Line' {
        Open-File $window $a 'open-goto-fixture'
        $before = Get-Frame $window top
        try {
            Send-Key $window @('Ctrl', 'G')
            $null = Wait-Changed $window $before top
            Send-Ascii $window '3'
            Save-Evidence $window 'goto-line'
            Send-Key $window Enter
            Send-Ascii $window X
            Send-Key $window @('Ctrl', 'S')
            Expect-Bytes $a "kiwi kiwi`r`nsecond`r`nXthird"
        } finally { $before.Dispose() }
    }
    Invoke-Probe 'Ctrl+D expands to two occurrences' {
        Send-Key $window @('Ctrl', 'Home')
        Send-Key $window @('Ctrl', 'D')
        Send-Key $window @('Ctrl', 'D')
        $before = Get-Frame $window
        try { Send-Ascii $window pear; $null = Wait-Changed $window $before client 80 }
        finally { $before.Dispose() }
        Send-Key $window @('Ctrl', 'S')
        Expect-Bytes $a "pear pear`r`nsecond`r`nXthird"
        Save-Evidence $window 'multi-select'
    }
    Invoke-Probe 'Ctrl+/ toggles Rust line comment' {
        Open-File $window $rust 'open-comment-fixture'
        Send-Key $window @('Ctrl', 'Slash')
        Send-Key $window @('Ctrl', 'S')
        Expect-Bytes $rust '// fn main() {}'
        Save-Evidence $window 'commented'
        Send-Key $window @('Ctrl', 'Slash')
        Send-Key $window @('Ctrl', 'S')
        Expect-Bytes $rust 'fn main() {}'
        Save-Evidence $window 'uncommented'
    }
    Invoke-Probe 'Ctrl+K Ctrl+B toggles sidebar' {
        $before = Get-Frame $window left
        try {
            Send-Key $window @('Ctrl', 'K')
            Send-Key $window @('Ctrl', 'B')
            $null = Wait-Changed $window $before left
            Save-Evidence $window 'sidebar-toggled'
            Send-Key $window @('Ctrl', 'K')
            Send-Key $window @('Ctrl', 'B')
            $null = Wait-Until 'sidebar restored' {
                $now = Get-Frame $window left
                try { [VelocityDesktop.Native]::Difference($before, $now) -lt 400 } finally { $now.Dispose() }
            }
            Save-Evidence $window 'sidebar-restored'
        } finally { $before.Dispose() }
    }
    Invoke-Probe 'F11 enters and leaves fullscreen' {
        $bounds = [VelocityDesktop.Native]::Outer($window)
        Send-Key $window F11
        $null = Wait-Until 'window is borderless fullscreen' { -not [VelocityDesktop.Native]::HasCaption($window) -and
            [VelocityDesktop.Native]::Outer($window).Width -gt $bounds.Width }
        Save-Evidence $window 'fullscreen'
        Send-Key $window F11
        $null = Wait-Until 'window restores original frame' { [VelocityDesktop.Native]::HasCaption($window) -and
            [VelocityDesktop.Native]::Outer($window) -eq $bounds }
        Save-Evidence $window 'windowed'
    }
    Invoke-Probe 'Ctrl+Tab cycles the most recently used tabs' {
        Open-File $window $a 'activate-first-tab'
        Open-File $window $b 'activate-second-tab'
        Send-Key $window @('Ctrl', 'Tab')
        $null = Wait-Title $window 'shortcut-a.txt'
        Save-Evidence $window 'tab-cycle'
    }
    Invoke-Probe 'Ctrl+W closes and Ctrl+Shift+T reopens' {
        Open-File $window $b 'activate-close-tab'
        Send-Key $window @('Ctrl', 'W')
        $null = Wait-Until 'closed tab no longer active' { [VelocityDesktop.Native]::Title($window) -notlike '*shortcut-b.txt*' }
        Save-Evidence $window 'tab-closed'
        Send-Key $window @('Ctrl', 'Shift', 'T')
        $null = Wait-Title $window 'shortcut-b.txt'
        Save-Evidence $window 'tab-reopened'
    }
}

function Test-ClipboardRoundTrip {
    $window = Start-Editor 'clipboard'
    $text = $fixture.cjk + ' paste'
    Set-Clipboard -Value $text
    Send-Key $window @('Ctrl', 'V')
    $null = Wait-Title $window ([string][char]0x2022)
    $copied = Read-Editor $window
    Assert-That ($copied -ceq $text) 'System clipboard to editor did not round-trip CJK'
    Save-Evidence $window 'paste-from-system'
    Send-Text $window ($fixture.cjk + ' copy')
    $copied = Read-Editor $window
    Assert-That ($copied -ceq ($fixture.cjk + ' copy')) 'Editor did not copy exact CJK to system clipboard'
    Save-Evidence $window 'copy-to-system'
}
