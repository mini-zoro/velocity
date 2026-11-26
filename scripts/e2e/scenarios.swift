// Scenario implementations for the GUI end-to-end harness.
//
// Each scenario owns one disposable data directory and its own Velocity process,
// asserts on the accessibility tree, the window list and files on disk, and ends
// with a SUMMARY line from Report.finish().
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

struct Environment {
    let work: URL
    let dataDir: URL
    let launcher: URL

    static func from(_ options: ArraySlice<String>) -> Environment {
        guard let work = options.first else { fail("scenario needs a work directory") }
        let dataDir = options.count > 1 ? options[1] : "\(work)/data"
        let launcher = ProcessInfo.processInfo.environment["E2E_VELOCITY"]
            ?? "\(work)/velocity-launcher"
        return Environment(work: URL(fileURLWithPath: work),
                           dataDir: URL(fileURLWithPath: dataDir),
                           launcher: URL(fileURLWithPath: launcher))
    }

    var files: URL { work.appendingPathComponent("files") }

    func fixture(_ name: String, contents: String = "") -> String {
        let url = files.appendingPathComponent(name)
        try? FileManager.default.createDirectory(at: files, withIntermediateDirectories: true)
        try? contents.write(to: url, atomically: true, encoding: .utf8)
        return url.path
    }

    func path(_ name: String) -> String {
        files.appendingPathComponent(name).path
    }

    func reset() {
        killVelocity(at: launcher)
        try? FileManager.default.removeItem(at: dataDir)
        try? FileManager.default.createDirectory(at: files, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
    }

    var executable: String {
        // <install>/bin/velocity -> <install>/MacOS/velocity when the launcher is the
        // packaging wrapper; a bare developer binary is used directly otherwise.
        let text = (try? String(contentsOf: launcher, encoding: .utf8)) ?? ""
        if text.contains("MacOS/velocity") || text.contains("VELOCITY_BIN_DIR") {
            return launcher.resolvingSymlinksInPath().deletingLastPathComponent()
                .appendingPathComponent("velocity").path
        }
        return launcher.path
    }
}

// MARK: - Process and window helpers

/// Ends a helper app without Apple events (Automation prompts would steal focus).
func killQuietly(_ name: String) {
    _ = shell("pkill -x \(name.shellQuoted) 2>/dev/null || true")
}

func killVelocity(at launcher: URL) {
    let target = launcher.resolvingSymlinksInPath().path
    _ = shell("pkill -f -- \(target.shellQuoted) 2>/dev/null; pkill -x velocity 2>/dev/null; true")
    sleep(seconds: 0.5)
}

extension String {
    var shellQuoted: String { "'" + replacingOccurrences(of: "'", with: "'\\''") + "'" }
}

/// Launches Velocity through its CLI with an isolated data directory.
/// `background` adds -b; `files` are paths.
@discardableResult
func launchVelocity(_ environment: Environment, files: [String], background: Bool = false,
                    extra: [String] = []) -> pid_t {
    // The runner's display is 1024x768; a window wider than the screen would put
    // drop targets off-screen.
    var arguments = ["--data-dir", environment.dataDir.path, "--width", "900", "--height", "620"]
    if background { arguments.insert("-b", at: 0) }
    arguments.append(contentsOf: extra)
    arguments.append(contentsOf: files)
    let process = Process()
    process.executableURL = environment.launcher
    process.arguments = arguments
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    process.standardInput = FileHandle.nullDevice
    do { try process.run() } catch { fail("cannot launch velocity: \(error)") }
    var pid: pid_t = 0
    // waitUntil, not a bare loop: AX reads refused while the app starts up are
    // retried, not left pending for the scenario's first check.
    _ = waitUntil(45, interval: 0.2) {
        guard let found = velocityPid(executable: environment.executable), !onScreenWindows(found).isEmpty,
              !velocityWindows(found).isEmpty else { return false }
        pid = found
        return true
    }
    if pid == 0 { fail("velocity did not present a window within 45 s") }
    // A window can exist before its first frame; wait for a second sampling.
    sleep(seconds: 0.4)
    return pid
}

func velocityPid(executable: String) -> pid_t? {
    topVelocityProcesses(executable: executable).first.map { pid_t($0.pid) }
}

/// Velocity's editor windows: standard windows only. AXWindows also lists the
/// input-method indicator bubble the system shows inside the app after IME typing
/// (macOS 14+), and lists it first; it has no text area and no tabs.
func velocityWindows(_ pid: pid_t) -> [AXUIElement] {
    axStandardWindows(pid)
}

/// The editor window a person is working in (main, else focused, else first
/// standard window; see `axDocumentWindow`), or the first standard window whose
/// title contains `contains`.
func velocityWindow(_ pid: pid_t, title contains: String? = nil) -> AXUIElement? {
    if let contains {
        return velocityWindows(pid).first { axWindowTitle($0).contains(contains) }
    }
    return axDocumentWindow(pid)
}

func windowTitles(_ pid: pid_t) -> [String] {
    velocityWindows(pid).map { axWindowTitle($0) }
}

/// AXRadioButton tab nodes under a window (the tab strip is a group of them).
func tabNodes(_ window: AXUIElement) -> [AXUIElement] {
    axFindAll(window, role: "AXRadioButton")
}

/// AX elements are recreated per query, so identity is compared by window frame.
func sameWindow(_ a: AXUIElement, _ b: AXUIElement) -> Bool {
    guard let a = axRect(a), let b = axRect(b) else { return false }
    return abs(a.minX - b.minX) < 2 && abs(a.minY - b.minY) < 2
        && abs(a.width - b.width) < 2 && abs(a.height - b.height) < 2
}

struct TabInfo {
    let label: String
    let selected: Bool
    let frame: CGRect
}

func tabs(_ window: AXUIElement) -> [TabInfo] {
    tabNodes(window).compactMap { node in
        guard let frame = axRect(node) else { return nil }
        return TabInfo(label: axString(node, kAXTitleAttribute) ?? axString(node, kAXDescriptionAttribute) ?? "",
                       selected: axBool(node, kAXSelectedAttribute) ?? (axBool(node, kAXValueAttribute) ?? false),
                       frame: frame)
    }
}

func tab(_ window: AXUIElement, named name: String) -> TabInfo? {
    tabs(window).first { $0.label.hasPrefix(name) }
}

/// The editor element, waiting briefly: right after a tab switch Velocity can rebuild
/// its accessibility snapshot and answer with no text area for a moment.
func editorNode(_ window: AXUIElement, timeout: Double = 4) -> AXUIElement? {
    var found: AXUIElement?
    _ = waitUntil(timeout) {
        found = axFindAll(window, role: "AXTextArea").first
        return found != nil
    }
    return found
}

/// The editor's AXValue, or nil with the reason. A missing text area or a failed
/// read is recorded as an AX error, so the next check cannot pass on it.
func editorValue(_ window: AXUIElement) -> (text: String?, why: String) {
    guard let node = editorNode(window) else {
        let why = "no AXTextArea in window \"\(axWindowTitle(window))\""
        AXReadErrors.shared.record(why)
        return (nil, why)
    }
    return axRequiredString(node, kAXValueAttribute)
}

/// `editorValue` for callers that compare strings; "" when unreadable (and recorded).
func editorText(_ window: AXUIElement) -> String {
    editorValue(window).text ?? ""
}

func selectedRange(_ window: AXUIElement) -> NSRange? {
    guard let node = editorNode(window) else { return nil }
    return axRange(node, kAXSelectedTextRangeAttribute)
}

/// Comma-joined selected text ranges, for multi-caret logs.
/// On-screen windows Velocity's process owns above layer 0 (input-method candidate
/// panels and indicators live there), plus what AXWindows lists.
func imeWindowSummary(_ pid: pid_t) -> String {
    let raw = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]]) ?? []
    let floating = raw.compactMap { entry -> String? in
        guard entry[kCGWindowOwnerPID as String] as? pid_t == pid,
              let layer = entry[kCGWindowLayer as String] as? Int, layer != 0,
              let bounds = entry[kCGWindowBounds as String] as? [String: Any],
              let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary) else { return nil }
        return "layer \(layer) \(Int(rect.minX)),\(Int(rect.minY)) \(Int(rect.width))x\(Int(rect.height))"
    }
    return "on-screen above layer 0: [\(floating.joined(separator: "; "))] AX: \(axWindowSummary(pid))"
}

/// Pinyin shows the preedit with syllable separators ("ni hao", "zhong'wen");
/// compare without them. Either the Latin preedit or a candidate's characters count.
func isComposing(_ text: String, pinyin: String = "zhongwen", hanzi: String = "中文") -> Bool {
    let squeezed = text.filter { $0 != " " && $0 != "'" && $0 != "\u{2019}" }
    return squeezed.contains(pinyin) || text.contains(hanzi)
}

/// A click point just inside the left edge of `range`'s glyph, from AXBoundsForRange,
/// or nil (with a FAIL saying why) when the rectangle is empty or lies outside the
/// text area. A rectangle in Cocoa's bottom-left coordinates is flipped and noted.
func textTarget(_ node: AXUIElement, area: CGRect, range: NSRange, report: Report) -> CGPoint? {
    guard let rect = axBoundsForRange(node, range), rect.width > 0, rect.height > 0 else {
        report.check(false, "AXBoundsForRange \(NSStringFromRange(range)) is a usable rectangle",
                     detail: String(describing: axBoundsForRange(node, range)))
        return nil
    }
    let point = CGPoint(x: rect.minX + min(2, rect.width / 4), y: rect.midY)
    if area.contains(point) { return point }
    let screenHeight = CGDisplayBounds(CGMainDisplayID()).height
    let flipped = CGPoint(x: point.x, y: screenHeight - rect.midY)
    if area.contains(flipped) {
        report.note("AXBoundsForRange came back in bottom-left coordinates", detail: "\(rect) flipped to \(flipped)")
        return flipped
    }
    report.check(false, "AXBoundsForRange \(NSStringFromRange(range)) lies inside the text area",
                 detail: "rect=\(rect) area=\(area)")
    return nil
}

/// Velocity exposes only the primary selection here, so this cannot count carets.
func describeRanges(_ element: AXUIElement) -> String {
    guard let ranges = axRanges(element) else {
        return "single:\(String(describing: axRange(element, kAXSelectedTextRangeAttribute)))"
    }
    return ranges.map { NSStringFromRange($0) }.joined(separator: " ")
}

func tabSummary(_ window: AXUIElement) -> String {
    tabs(window).map { "\($0.label)\($0.selected ? "*" : "")" }.joined(separator: ",")
}

/// True when the window is in the on-screen window list (a real frame, not just an
/// NSWindow that AppKit keeps around).
func isOnScreen(_ window: AXUIElement, pid: pid_t) -> Bool {
    guard let frame = axRect(window) else { return false }
    return onScreenWindows(pid).contains { $0.bounds.intersects(frame.insetBy(dx: 4, dy: 4)) }
}

func bringToFront(_ pid: pid_t, report: Report) {
    if frontmostPid() != pid {
        _ = activate(pid)
        _ = waitUntil(3) { frontmostPid() == pid }
    }
    if frontmostPid() != pid {
        report.note("activate", detail: "AX activation refused; a HID click is needed (cooperative activation)")
    }
}

/// Clicks a point inside a window and refuses to do so when the point is not over
/// one of this process's windows (the pointer goes to whatever is under it).
func guardedClick(_ point: CGPoint, pid: pid_t, report: Report, label: String, clicks: Int = 1,
                  flags: CGEventFlags = []) -> Bool {
    guard windowList().contains(where: { $0.pid == pid && $0.bounds.contains(point) }) else {
        report.check(false, "\(label): click target is inside a Velocity window",
                     detail: "\(Int(point.x)),\(Int(point.y)) over \(windowUnder(point))")
        return false
    }
    click(at: point, clicks: clicks, flags: flags)
    return true
}

func windowUnder(_ point: CGPoint) -> String {
    let hit = windowList(layers: [0, 8, 25]).last { $0.bounds.contains(point) }
    return hit.map { "\($0.owner)#\($0.id)" } ?? "none"
}

/// Places two windows side by side so neither covers the other's tab strip.
func arrangeSideBySide(_ first: AXUIElement, _ second: AXUIElement, report: Report) {
    let ok = moveWindow(first, to: CGRect(x: 10, y: 20, width: 480, height: 460))
        && moveWindow(second, to: CGRect(x: 520, y: 20, width: 480, height: 460))
    report.note("window arrangement", detail: ok ? "side by side" : "AX refused the move")
    sleep(seconds: 0.4)
}

func dismissAlerts(report: Report) {
    // A runner session can show a TCC or "bypass the window picker" alert; it takes
    // focus away from every app under test.
    for owner in ["UserNotificationCenter", "CoreServicesUIAgent", "SecurityAgent", "screencaptureui"] {
        guard let raw = output("pgrep -x \(owner)"), let pid = pid_t(raw) else { continue }
        let app = axApp(pid)
        // Other apps' trees: refusals there are not Velocity's and decide nothing.
        let buttons = AXReadErrors.shared.quietly { axFindAll(app, role: "AXButton") }
        for button in buttons where ["Allow", "OK", "Cancel", "Don't Allow", "不允许", "允许"]
            .contains(AXReadErrors.shared.quietly { axString(button, kAXTitleAttribute) } ?? "") {
            _ = AXUIElementPerformAction(button, kAXPressAction as CFString)
            report.note("alert dismissed", detail: "\(owner) \(AXReadErrors.shared.quietly { axString(button, kAXTitleAttribute) } ?? "")")
        }
    }
}

/// Dumps the accessibility tree and a screen shot for the uploaded artifacts.
func capture(_ report: Report, pid: pid_t, label: String) {
    dumpOnFailure(report, pid: pid, label: label)
}

func dumpOnFailure(_ report: Report, pid: pid_t, label: String) {
    guard let dir = ProcessInfo.processInfo.environment["E2E_ARTIFACTS"] else { return }
    let path = "\(dir)/ax-\(label)-\(pid).txt"
    try? axDumpFile(pid: pid, to: path)
    _ = shell("mkdir -p \(dir.shellQuoted) && for w in $(screencapture -x -o /dev/null 2>/dev/null; true); do :; done")
    let shot = "\(dir)/screen-\(label).png"
    _ = shell("screencapture -x \(shot.shellQuoted) 2>/dev/null || true")
    report.note("artifacts", detail: "\(path) \(shot)")
}

// MARK: - Scenario 1: Pinyin IME in the editor

func scenarioIME(_ options: ArraySlice<String>) -> Never {
    let report = Report("ime")
    let environment = Environment.from(options)
    environment.reset()
    let a = environment.fixture("a.txt", contents: "")
    let b = environment.fixture("b.txt", contents: "second\n")
    let c = environment.fixture("c.txt", contents: "third\n")
    // Pinyin first: an input context created before the switch keeps the layout.
    _ = switchToPinyin(report)
    let pid = launchVelocity(environment, files: [a, b, c])
    dismissAlerts(report: report)
    report.info("pid", detail: "\(pid) windows=\(windowTitles(pid)) tabs=\(tabSummary(velocityWindow(pid)!))")

    let pinyin = switchToPinyin(report)
    report.check(pinyin, "Pinyin is the current input source", detail: currentInputSourceId())
    guard pinyin else { report.finish() }
    _ = pinyin

    guard let window = velocityWindow(pid), let editor = editorNode(window) else {
        fail("no editor element; AX windows: \(axWindowSummary(pid))")
    }
    bringToFront(pid, report: report)
    if frontmostPid() != pid {
        // Cooperative activation can refuse a background launch; one click on the
        // window is the user's own path and is how a person would start typing.
        if let frame = axRect(window) {
            _ = guardedClick(CGPoint(x: frame.midX, y: frame.minY + 120), pid: pid, report: report, label: "focus")
        }
    }
    report.check(frontmostPid() == pid, "Velocity frontmost before typing", detail: frontmostName())

    // Every text below is compared exactly against what the fixture and the typing
    // make it, so an unreadable or wrong editor can never pass as "no preedit".
    let cText = "你好third\n"
    let bFixture = "second\n"
    let cBefore = editorText(window)
    report.check(cBefore == "third\n", "the editor shows c.txt, the last file opened", detail: cBefore.debugDescription)

    // 1a. nihao + space -> 你好. The preedit must show in AXValue while composing:
    // the "no preedit left" checks below mean nothing otherwise.
    _ = AXUIElementSetAttributeValue(editor, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    typeKeys("nihao", gap: 55_000)
    let preview = waitUntil(3) { isComposing(editorText(window), pinyin: "nihao", hanzi: "你") }
    report.check(preview, "the preedit is exposed through AXValue while composing", detail: editorText(window).debugDescription)
    typeKeys(" ", gap: 55_000)
    let committed = waitUntil(5) { editorText(window) == cText }
    report.check(committed, "nihao + space inserts 你好", detail: editorText(window).debugDescription)

    // 1b. zhongwen + Escape leaves the text exactly as it was.
    postKey("a", flags: .maskCommand)
    usleep(120_000)
    typeKeys("zhongwen", gap: 55_000)
    let composingB = waitUntil(3) { isComposing(editorText(window)) }
    report.check(composingB, "composition is live before Escape", detail: editorText(window).debugDescription)
    postKey("escape")
    let escaped = waitUntil(3) { editorText(window) == cText }
    report.check(escaped, "zhongwen + Escape inserts nothing and leaves no preedit", detail: editorText(window).debugDescription)

    // 1c. Cmd+1 while composing is swallowed (keyDown/performKeyEquivalent drop
    // Command keys while marked text is live, and the menu does not fall back):
    // the tab does not change.
    postKey("2", flags: .maskCommand)
    let onB = waitUntil(3) { tab(window, named: "b.txt")?.selected == true }
    report.check(onB, "Cmd+2 selects b.txt (no composition live)", detail: tabSummary(window))
    let bShown = editorValue(window)
    report.check(bShown.text == bFixture, "the editor shows b.txt's file text", detail: bShown.text?.debugDescription ?? bShown.why)
    let bLabel = tab(window, named: "b.txt")?.label ?? ""
    typeKeys("zhongwen", gap: 55_000)
    let composingC = waitUntil(3) { isComposing(editorText(window)) }
    report.check(composingC, "composition is live in b.txt", detail: editorText(window).debugDescription)
    // What AXWindows holds now: the input-method indicator bubble is listed before
    // the editor window while it is up.
    report.info("AX windows while composing", detail: axWindowSummary(pid))
    postKey("1", flags: .maskCommand)
    let leftB = waitUntil(1.0) { tab(window, named: "a.txt")?.selected == true || tab(window, named: "b.txt")?.selected != true }
    report.check(!leftB && tab(window, named: "b.txt")?.selected == true,
                 "Cmd+1 during a composition is swallowed: b.txt stays selected", detail: tabSummary(window))
    report.note("composition after the swallowed Cmd+1", detail: editorText(window).debugDescription)
    if leftB {
        postKey("2", flags: .maskCommand)
        _ = waitUntil(3) { tab(window, named: "b.txt")?.selected == true }
    }
    if !isComposing(editorText(window)) {
        typeKeys("zhongwen", gap: 55_000)
        _ = waitUntil(3) { isComposing(editorText(window)) }
    }
    let liveBeforeClick = editorText(window)
    report.check(isComposing(liveBeforeClick), "composition is live in b.txt before clicking another tab",
                 detail: liveBeforeClick.debugDescription)

    // 1d. Leaving a surface with a live composition discards it (documented). A
    // person leaves by clicking another tab: the click switches, the preedit lands
    // nowhere, and the tab that was left keeps its text.
    if let cTab = tab(window, named: "c.txt") {
        _ = guardedClick(CGPoint(x: cTab.frame.midX, y: cTab.frame.midY), pid: pid, report: report, label: "c.txt tab")
    } else {
        report.check(false, "tab c.txt found for the click", detail: tabSummary(window))
    }
    let onC = waitUntil(4) { tab(window, named: "c.txt")?.selected == true }
    report.info("AX windows after the click", detail: axWindowSummary(pid))
    capture(report, pid: pid, label: "ime-after-tab-click")
    report.check(onC && tab(window, named: "b.txt")?.selected == false,
                 "clicking c.txt during the composition selects it", detail: tabSummary(window))
    let areaAfterClick = editorNode(window)
    report.check(areaAfterClick != nil, "the editor text area is in the editor window after the click",
                 detail: "window=\(axWindowTitle(window).debugDescription) all=\(axWindowSummary(pid))")
    let cShown = editorValue(window)
    report.check(cShown.text == cText, "c.txt shows exactly its own text: no preedit carried over",
                 detail: cShown.text?.debugDescription ?? cShown.why)
    let bLabelAfter = tab(window, named: "b.txt")?.label ?? ""
    report.check(!bLabel.isEmpty && bLabelAfter == bLabel, "b.txt is not marked edited after being left mid-composition",
                 detail: "before=\(bLabel.debugDescription) after=\(bLabelAfter.debugDescription)")
    // What the input method itself still holds. Velocity's preedit is gone (above),
    // but the candidate panel was seen lingering after the click; the next key shows
    // whether the abandoned composition is still alive and follows into c.txt.
    sleep(seconds: 1.5)
    report.info("IME windows 2 s after the click", detail: imeWindowSummary(pid))
    capture(report, pid: pid, label: "ime-2s-after-tab-click")
    typeKeys("a", gap: 55_000)
    _ = waitUntil(2) { editorText(window) != cText }
    let afterKey = editorText(window)
    report.check(afterKey != cText, "the next key in c.txt shows a preedit", detail: afterKey.debugDescription)
    if afterKey != cText {
        report.check(!isComposing(afterKey), "the abandoned composition does not follow into c.txt",
                     detail: "\(afterKey.debugDescription) \(imeWindowSummary(pid))")
    }
    postKey("escape")
    let cRestored = waitUntil(3) { editorText(window) == cText }
    report.check(cRestored, "Escape then leaves c.txt exactly as it was", detail: editorText(window).debugDescription)
    postKey("2", flags: .maskCommand)
    let backOnB = waitUntil(3) { tab(window, named: "b.txt")?.selected == true }
    let bAfter = editorValue(window)
    report.check(backOnB && bAfter.text == bFixture, "b.txt still holds exactly its file text",
                 detail: "selected=\(backOnB) text=\(bAfter.text?.debugDescription ?? bAfter.why)")
    postKey("s", flags: .maskCommand)
    sleep(seconds: 1.0)
    let bOnDisk = (try? String(contentsOfFile: b, encoding: .utf8)) ?? "<unreadable>"
    report.check(bOnDisk == bFixture, "b.txt saved after the round trip is unchanged on disk", detail: bOnDisk.debugDescription)

    // 1e. Two carets (Ctrl+Shift+Down), then ceshi + space inserts 测试 at both and
    // one Cmd+Z removes both. AX exposes only the primary selection, so the carets
    // are proven by the text, not by AXSelectedTextRanges.
    let lines = "line one\nline two\nline three"
    guard let multiEditor = editorNode(window) else {
        report.check(false, "editor text area for the multi-caret step", detail: axWindowSummary(pid))
        dumpOnFailure(report, pid: pid, label: "ime")
        killVelocity(at: environment.launcher)
        report.finish()
    }
    _ = AXUIElementSetAttributeValue(multiEditor, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    postKey("a", flags: .maskCommand)
    postKey("delete")
    usleep(200_000)
    typeUnicode("line one")
    postKey("return")
    typeUnicode("line two")
    postKey("return")
    typeUnicode("line three")
    let typed = waitUntil(3) { editorText(window) == lines }
    report.check(typed, "three plain lines typed with Pinyin active", detail: editorText(window).debugDescription)
    postKey("a", flags: .maskCommand)
    usleep(120_000)
    postKey("left")
    usleep(150_000)
    postKey("down", flags: [.maskControl, .maskShift])
    usleep(250_000)
    report.note("primary selection after Ctrl+Shift+Down", detail: describeRanges(multiEditor))
    let beforeMulti = editorText(window)
    typeKeys("ceshi", gap: 55_000)
    usleep(250_000)
    typeKeys(" ", gap: 55_000)
    let expectedMulti = "测试line one\n测试line two\nline three"
    let multi = waitUntil(3) { editorText(window) == expectedMulti }
    report.check(multi, "ceshi + space commits 测试 at both carets", detail: editorText(window).debugDescription)
    postKey("z", flags: .maskCommand)
    let undone = waitUntil(3) { editorText(window) == beforeMulti }
    report.check(undone, "one Cmd+Z removes both commits", detail: editorText(window).debugDescription)

    // 1f. Cmd+click adds a caret. Click targets come from AXBoundsForRange, so a
    // miss is a real miss rather than guessed font metrics; the outcome is read from
    // the text. Offsets are UTF-16: the space after "line" is 4 on line one, 22 on
    // line three; clicking a character's left edge puts the caret before it.
    postKey("a", flags: .maskCommand)
    postKey("delete")
    usleep(150_000)
    typeUnicode("line one")
    postKey("return")
    typeUnicode("line two")
    postKey("return")
    typeUnicode("line three")
    _ = waitUntil(3) { editorText(window) == lines }
    if let node = editorNode(window), let area = axRect(node),
       let third = textTarget(node, area: area, range: NSRange(location: 22, length: 1), report: report),
       let first = textTarget(node, area: area, range: NSRange(location: 4, length: 1), report: report) {
        _ = guardedClick(third, pid: pid, report: report, label: "caret 1")
        _ = guardedClick(first, pid: pid, report: report, label: "caret 2", flags: .maskCommand)
        usleep(250_000)
        typeUnicode("XY")
        let expected = "lineXY one\nline two\nlineXY three"
        let placed = waitUntil(3) { editorText(window) == expected }
        report.check(placed, "Cmd+click places a second caret",
                     detail: "\(editorText(window).debugDescription) clicks=\(Int(third.x)),\(Int(third.y)) \(Int(first.x)),\(Int(first.y))")
    } else {
        report.check(false, "AXBoundsForRange gives click targets inside the text area",
                     detail: editorNode(window).map(axDescribe) ?? "no text area")
    }

    // 1g. Rename-tab field composes immediately.
    _ = switchToPinyin(report)
    if let tabInfo = tab(window, named: "b.txt") {
        let point = CGPoint(x: tabInfo.frame.midX, y: tabInfo.frame.midY)
        _ = guardedClick(point, pid: pid, report: report, label: "tab title", clicks: 2)
        usleep(400_000)
        let field = axFindAll(window, role: "AXTextField").first {
            (axString($0, kAXDescriptionAttribute) ?? "").contains("Rename")
                || (axString($0, kAXTitleAttribute) ?? "") == "Rename"
        }
        report.check(field != nil, "double-clicking a tab opens the rename field",
                     detail: field.map { axDescribe($0) } ?? "none")
        if let field {
            postKey("a", flags: .maskCommand)
            typeKeys("ni", gap: 60_000)
            usleep(500_000)
            let read = axRequiredString(field, kAXValueAttribute)
            let value = read.text ?? ""
            report.check(isComposing(value, pinyin: "ni", hanzi: "你"),
                         "rename field shows the composed character", detail: read.text?.debugDescription ?? read.why)
            postKey("escape")
            usleep(200_000)
        } else {
            dumpOnFailure(report, pid: pid, label: "rename")
        }
    } else {
        report.check(false, "tab b.txt found for the rename check", detail: tabSummary(window))
    }

    _ = useInputSource(abcIds)
    if report.failures > 0 { dumpOnFailure(report, pid: pid, label: "ime") }
    killVelocity(at: environment.launcher)
    report.finish()
}

// MARK: - Scenario 2: tab drag out and merge

func scenarioTabs(_ options: ArraySlice<String>) -> Never {
    let report = Report("tabs")
    let environment = Environment.from(options)
    environment.reset()
    let a = environment.fixture("a.txt", contents: "one\n")
    let b = environment.fixture("b.txt", contents: "two\n")
    let c = environment.fixture("c.txt", contents: "three\n")
    let pid = launchVelocity(environment, files: [a, b, c])
    dismissAlerts(report: report)
    guard let source = velocityWindow(pid) else { fail("no source window") }
    bringToFront(pid, report: report)
    if frontmostPid() != pid, let frame = axRect(source) {
        _ = guardedClick(CGPoint(x: frame.midX, y: frame.minY + 120), pid: pid, report: report, label: "focus")
    }
    _ = moveWindow(source, to: CGRect(x: 20, y: 30, width: 900, height: 640))
    sleep(seconds: 0.3)
    _ = waitUntil(10) { !tabs(source).isEmpty }
    let initialTabs = tabs(source)
    report.info("start", detail: "tabs=\(tabSummary(source)) windows=\(onScreenWindows(pid).count)")
    report.check(initialTabs.count == 3, "three tabs in one window", detail: tabSummary(source))

    // 2a. Drag the third tab well below the strip: a new window appears.
    guard let dragged = tab(source, named: "c.txt"), let frame = axRect(source) else {
        report.check(false, "c.txt tab present"); report.finish()
    }
    let grab = CGPoint(x: dragged.frame.midX, y: dragged.frame.midY)
    let drop = CGPoint(x: frame.midX, y: min(frame.maxY + 140, (NSScreen.main?.frame.height ?? 1000) - 60))
    let before = Set(onScreenWindows(pid).map { $0.id })
    drag(from: grab, to: drop, steps: 16, pause: 0.08)
    usleep(500_000)
    let after = onScreenWindows(pid)
    report.info("windows after drag-out", detail: after.map { "\($0.id)@\(Int($0.bounds.minX)),\(Int($0.bounds.minY)) \(Int($0.bounds.width))x\(Int($0.bounds.height))" }.joined(separator: " "))
    let newWindows = after.filter { !before.contains($0.id) }
    report.check(!newWindows.isEmpty, "drag below the strip opens a new window")
    let newWindow = velocityWindows(pid).first { win in
        guard let r = axRect(win) else { return false }
        return newWindows.contains { $0.bounds.intersects(r.insetBy(dx: 6, dy: 6)) }
    }
    // The torn-off window paints its strip a moment after the drop.
    _ = waitUntil(5) { newWindow.map { tabs($0).contains { $0.label.hasPrefix("c.txt") } } ?? false }
    report.check(newWindow.map { tabs($0).contains { $0.label.hasPrefix("c.txt") } } ?? false,
                 "the new window holds c.txt", detail: newWindow.map { tabSummary($0) } ?? "no AX window")
    if let source = velocityWindows(pid).first(where: { win in !sameWindow(win, newWindow!) }) {
        report.check(!tabs(source).contains { $0.label.hasPrefix("c.txt") },
                     "the source window lost c.txt", detail: tabSummary(source))
    }

    // The torn-off window opens on top of the source strip; move it clear (its own
    // strip stays reachable) so the pointer reaches the source for the merge drag.
    if let newWindow {
        let moved = moveWindow(newWindow, to: CGRect(x: 20, y: 420, width: 700, height: 300))
        report.note("torn-off window", detail: "moved=\(moved)")
        _ = waitUntil(5) { !tabs(newWindow).isEmpty }
        sleep(seconds: 0.3)
    }
    // 2b. Drag the tab from the new window into the source strip: merge, source closes.
    if let newWindow, let mainWindow = velocityWindows(pid).first(where: { win in !sameWindow(win, newWindow) }),
       let moved = tab(newWindow, named: "c.txt"),
       // Dropping on an existing tab lands inside the strip; the window's top edge
       // is the title bar, not the strip.
       let target = tabs(mainWindow).first?.frame {
        let from = CGPoint(x: moved.frame.midX, y: moved.frame.midY)
        let to = CGPoint(x: target.midX, y: target.midY)
        drag(from: from, to: to, steps: 18, pause: 0.06)
        usleep(600_000)
        let windowsNow = velocityWindows(pid).filter { isOnScreen($0, pid: pid) }
        let merged = windowsNow.first { tabs($0).contains { $0.label.hasPrefix("c.txt") } }
        report.check(merged != nil, "dragging into the other strip merges the tab")
        report.check(windowsNow.count == 1, "the emptied source window closes", detail: "\(windowsNow.count) window(s)")
        report.check(windowsNow.first.map { tabs($0).count == 3 } ?? false,
                     "merged window keeps three tabs", detail: windowsNow.first.map { tabSummary($0) } ?? "")
    } else {
        report.check(false, "second window and its c.txt tab found for the merge")
    }

    // 2c. Drag out and press Escape: nothing changes.
    guard let window = velocityWindows(pid).first(where: { isOnScreen($0, pid: pid) }), let frame = axRect(window) else {
        report.finish()
    }
    let windowCount = velocityWindows(pid).filter { isOnScreen($0, pid: pid) }.count
    let tabCount = tabs(window).count
    if let victim = tab(window, named: "b.txt") {
        let start = CGPoint(x: victim.frame.midX, y: victim.frame.midY)
        let end = CGPoint(x: frame.midX, y: min(frame.maxY + 140, (NSScreen.main?.frame.height ?? 1000) - 60))
        mouseMove(to: start)
        mouseEvent(.leftMouseDown, start)
        for step in 1...16 {
            let point = CGPoint(x: start.x + (end.x - start.x) * CGFloat(step) / 16,
                                y: start.y + (end.y - start.y) * CGFloat(step) / 16)
            mouseEvent(.leftMouseDragged, point)
            Thread.sleep(forTimeInterval: 0.05)
        }
        postKey("escape")
        usleep(200_000)
        mouseEvent(.leftMouseUp, end)
        usleep(600_000)
        let now = velocityWindows(pid).filter { isOnScreen($0, pid: pid) }
        report.check(now.count == windowCount, "Escape during a drag creates no window", detail: "\(now.count) vs \(windowCount)")
        report.check(now.first.map { tabs($0).count == tabCount } ?? false,
                     "Escape during a drag keeps the tab in place", detail: now.first.map { tabSummary($0) } ?? "")
    }

    if report.failures > 0 { dumpOnFailure(report, pid: pid, label: "tabs") }
    killVelocity(at: environment.launcher)
    report.finish()
}

// MARK: - Scenario 3: first click on an inactive window's tab

func scenarioActivate(_ options: ArraySlice<String>) -> Never {
    let report = Report("activate")
    let environment = Environment.from(options)
    environment.reset()
    let a = environment.fixture("a.txt", contents: "one\n")
    let b = environment.fixture("b.txt", contents: "two\n")
    let c = environment.fixture("c.txt", contents: "three\n")
    let pid = launchVelocity(environment, files: [a, b, c])
    dismissAlerts(report: report)
    guard let window = velocityWindow(pid) else { fail("no window") }
    _ = moveWindow(window, to: CGRect(x: 20, y: 20, width: 900, height: 640))
    sleep(seconds: 0.3)
    // The last file named on the command line is the current tab.
    report.check(tab(window, named: "c.txt")?.selected ?? false, "the last launched file is current",
                 detail: tabSummary(window))

    // Another app takes the front, then one click on a non-current Velocity tab.
    let other = environment.fixture("other.txt", contents: "other\n")
    _ = shell("open -e \(other.shellQuoted)")
    guard let otherPid = waitForApp(bundleId: "com.apple.TextEdit", timeout: 15) else {
        report.check(false, "TextEdit came to the front"); report.finish()
    }
    // Keep TextEdit clear of Velocity's tab strip, or the click below hits TextEdit.
    if let editWindow = axDocumentWindow(otherPid) {
        _ = moveWindow(editWindow, to: CGRect(x: 560, y: 430, width: 440, height: 320))
    }
    _ = waitUntil(8) { frontmostPid() == otherPid }
    report.check(frontmostPid() == otherPid, "another app is frontmost", detail: frontmostName())
    guard let target = tab(window, named: "a.txt") else { report.check(false, "a.txt tab"); report.finish() }
    let point = CGPoint(x: target.frame.midX, y: target.frame.midY)
    report.note("click target", detail: "\(Int(point.x)),\(Int(point.y)) topmost=\(windowUnder(point)) front=\(frontmostName())")
    // The first click into an inactive window is the case under test; a synthesized
    // click is occasionally lost entirely, so a second identical click is allowed and
    // the per-click outcome is recorded rather than assumed.
    func tabSelected() -> Bool {
        guard let window = velocityWindow(pid) else { return false }
        return tabs(window).first { $0.label.hasPrefix("a.txt") }?.selected ?? false
    }
    var attempts = 0
    var firstSelected = false
    var firstActivated = false
    for attempt in 1...2 {
        attempts = attempt
        _ = guardedClick(point, pid: pid, report: report, label: "inactive tab", clicks: 1)
        let selected = waitUntil(2) { tabSelected() }
        let active = waitUntil(2) { frontmostPid() == pid }
        if attempt == 1 { firstSelected = selected; firstActivated = active }
        if selected && active { break }
    }
    report.note("click outcomes", detail: "attempts=\(attempts) first: selected=\(firstSelected) activated=\(firstActivated)")
    report.check(tabSelected(), "a click on the non-current tab selects it",
                 detail: velocityWindow(pid).map { tabSummary($0) } ?? "no window")
    report.check(frontmostPid() == pid, "a click on the inactive window activates Velocity",
                 detail: "\(attempts) click(s), front=\(frontmostName())")
    if frontmostPid() != pid { dumpOnFailure(report, pid: pid, label: "activate-click") }
    if report.failures > 0 { dumpOnFailure(report, pid: pid, label: "activate") }
    killQuietly("TextEdit")
    killVelocity(at: environment.launcher)
    report.finish()
}

// MARK: - Scenario 4: velocity -b does not come to the front

func scenarioBackground(_ options: ArraySlice<String>) -> Never {
    let report = Report("background")
    let environment = Environment.from(options)
    environment.reset()
    let other = environment.fixture("other.txt", contents: "other\n")
    let file = environment.fixture("cold.txt", contents: "cold\n")
    _ = shell("open -e \(other.shellQuoted)")
    guard let otherPid = waitForApp(bundleId: "com.apple.TextEdit", timeout: 15) else {
        report.check(false, "TextEdit came to the front"); report.finish()
    }
    _ = waitUntil(8) { frontmostPid() == otherPid }
    report.check(frontmostPid() == otherPid, "another app is frontmost before -b", detail: frontmostName())
    let pid = launchVelocity(environment, files: [file], background: true)
    // Give a stray activation a moment to happen before judging.
    sleep(seconds: 2.0)
    report.check(frontmostPid() == otherPid, "velocity -b leaves the other app frontmost",
                 detail: "front=\(frontmostName()) velocity=\(pid)")
    report.check(!windowTitles(pid).isEmpty, "velocity -b still opens the file", detail: "\(windowTitles(pid))")
    if report.failures > 0 { dumpOnFailure(report, pid: pid, label: "background") }
    killQuietly("TextEdit")
    killVelocity(at: environment.launcher)
    report.finish()
}

// MARK: - Scenario 5: footprint report (informational)

func scenarioFootprint(_ options: ArraySlice<String>) -> Never {
    let report = Report("footprint")
    let environment = Environment.from(options)
    environment.reset()
    let files = (1...5).map { environment.fixture("w\($0).txt", contents: String(repeating: "line \($0)\n", count: 400)) }
    let pid = launchVelocity(environment, files: [files[0]])
    dismissAlerts(report: report)
    usleep(1_500_000)
    func footprint() -> UInt64 {
        footprintBytes(pid) ?? 0
    }
    let one = footprint()
    report.note("footprint 1 window", detail: "\(onScreenWindows(pid).count) window(s) \(one / 1_048_576) MiB (\(one) B)")
    for extra in files.dropFirst() {
        _ = shell("\(environment.launcher.path.shellQuoted) --data-dir \(environment.dataDir.path.shellQuoted) -n \(extra.shellQuoted)")
        _ = waitUntil(8) { onScreenWindows(pid).count > 0 }
        usleep(500_000)
    }
    _ = waitUntil(15) { onScreenWindows(pid).count >= 5 }
    usleep(1_500_000)
    let five = footprint()
    let windows = onScreenWindows(pid).count
    report.note("footprint 5 windows", detail: "\(windows) window(s) \(five / 1_048_576) MiB (\(five) B)")
    let delta = one > 0 && five > one ? Double(five - one) / Double(one) : 0
    report.note("summary", detail: "1 window \(one / 1_048_576) MiB, 5 windows \(five / 1_048_576) MiB, +\(String(format: "%.1f", delta * 100))%")
    killVelocity(at: environment.launcher)
    report.finish()
}
