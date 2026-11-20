# Round 25: Simplified Chinese editor chrome

Scope: section D of [the UI localization spec](../../specs/vel-ui-menus-settings-i18n.md),
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
