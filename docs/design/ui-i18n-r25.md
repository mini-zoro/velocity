# Round 25: Simplified Chinese editor chrome

Scope: section D of the UI localization spec,
following the visual checks in [design round 5](ui-audit-r5.md). The User preference
still defaults to English. Locale and catalog changes invalidate prepared labels
and retained messages rather than reopening panels, discarding the search, or
rewriting document text.

## Terminology

Use 查找 / 替换 / 位置, 打开的文件 / 文件夹, 行 / 列, 空格 / 制表符宽度,
and 已选择 N 个字符. The sidebar keeps its heading font, size and weight but
does not apply an uppercase transform in Simplified Chinese. Alert button order
and actions are unchanged; the unsaved-document buttons are 保存 / 不保存 / 取消.

Keep the line-ending identifiers **Unix**, **Windows**, and **Mac OS 9** in both
languages. The popup and status bar use the same names; they mean LF, CRLF and
CR respectively. Apple's [macOS line-ending reference](https://developer.apple.com/library/archive/documentation/OpenSource/Conceptual/ShellScripting/PortingScriptstoMacOSX/PortingScriptstoMacOSX.html)
distinguishes these formats by platform.
[CotEditor's Simplified Chinese catalog](https://github.com/coteditor/CotEditor/blob/main/Packages/EditorCore/Sources/LineEnding/Resources/Localizable.xcstrings)
keeps its own platform labels unchanged, including Windows and macOS / Unix.
Velocity follows the same convention with its existing labels.

Other verbatim data includes filenames, paths, project names, syntax/scope names,
setting keys, command identifiers, encoding names, appearance names, Git branch
names and badge symbols, compiler/subprocess output, and user/package content.
System-owned controls and diagnostics keep their system text.

## Render handoff

Run in a fixture profile. `--scale` is display density; the two `_ui2` scenes
independently set `ui_scale=2`. The regular `r5_*` cases use UI scale 1.

```sh
env HOME=/tmp/velocity-r25-home SHELL=/bin/sh target/release/velocity \
  --render-scenes /tmp/velocity-r25-metal --language zh-Hans --scale 1,2 \
  --only r5_find,r5_find_in_files,r5_sidebar,r5_input_panel,r25_find_ui2,r25_find_in_files_ui2,r25_status_selection,r25_build_output
```

The same cases work with `--language en` for comparison. Append
`--bench-scene 1` to build and measure the real scenes without Metal.

## Review renders (Metal)

Rendered through the real Metal path by the reviewer, `--language zh-Hans`:
`after/r25-find-zh-dark@2x.png`, `r25-find_in_files-zh-dark@2x.png`, `r25-sidebar-zh-dark@2x.png`,
`r25-input_panel-zh-dark@2x.png`, `r25-status_selection-zh-dark@2x.png`,
`r25-build_output-zh-dark@2x.png`, and at UI scale 2 `r25-find_ui2-zh-light@1x.png`,
`r25-find_in_files_ui2-zh-light@1x.png`.

Review fixes: the `r25_build_output` scene now configures the output view as a build does
(it showed an empty panel before), and the output view now uses the editor's measured wide
advances, so the localized "[已完成，用时 1.2s]" footer and Chinese tool output are no longer
letter-spaced (`round26_settings_tests::output_view_uses_the_measured_wide_advances`).
