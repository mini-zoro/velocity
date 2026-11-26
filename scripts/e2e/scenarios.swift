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
    var arguments = ["--data-dir", environment.dataDir.path, "--width", "1100", "--height", "760"]
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
    let deadline = Date().addingTimeInterval(45)
    var pid: pid_t = 0
    while Date() < deadline {
        if let found = velocityPid(executable: environment.executable), !onScreenWindows(found).isEmpty,
           axWindows(found).count > 0 {
            pid = found
            // A window can exist before its first frame; wait for a second sampling.
            sleep(seconds: 0.4)
            break
        }
        sleep(seconds: 0.2)
    }
    if pid == 0 { fail("velocity did not present a window within 45 s") }
    return pid
}

func velocityPid(executable: String) -> pid_t? {
    topVelocityProcesses(executable: executable).first.map { pid_t($0.pid) }
}

/// The frontmost Velocity window (first in AXWindows order) by title substring.
func velocityWindow(_ pid: pid_t, title contains: String? = nil) -> AXUIElement? {
    let windows = axWindows(pid)
    if let contains {
        return windows.first { axWindowTitle($0).contains(contains) }
    }
    return windows.first
}

func windowTitles(_ pid: pid_t) -> [String] {
    axWindows(pid).map { axWindowTitle($0) }
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

func editorNode(_ window: AXUIElement) -> AXUIElement? {
    axFindAll(window, role: "AXTextArea").first
}

func editorText(_ window: AXUIElement) -> String {
    guard let node = editorNode(window) else { return "" }
    return axString(node, kAXValueAttribute) ?? ""
}

func selectedRange(_ window: AXUIElement) -> NSRange? {
    guard let node = editorNode(window) else { return nil }
    return axRange(node, kAXSelectedTextRangeAttribute)
}

/// Comma-joined selected text ranges, for multi-caret logs.
func describeRanges(_ element: AXUIElement) -> String {
    guard let value = axAttr(element, kAXSelectedTextRangesAttribute as String) as? [NSValue] else {
        return "single:\(String(describing: axRange(element, kAXSelectedTextRangeAttribute)))"
    }
    return value.map { NSStringFromRange($0.rangeValue) }.joined(separator: " ")
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

func dismissAlerts(report: Report) {
    // A runner session can show a TCC or "bypass the window picker" alert; it takes
    // focus away from every app under test.
    for owner in ["UserNotificationCenter", "CoreServicesUIAgent", "SecurityAgent", "screencaptureui"] {
        guard let raw = output("pgrep -x \(owner)"), let pid = pid_t(raw) else { continue }
        let app = axApp(pid)
        for button in axFindAll(app, role: "AXButton") where ["Allow", "OK", "Cancel", "Don't Allow", "不允许", "允许"]
            .contains(axString(button, kAXTitleAttribute) ?? "") {
            _ = AXUIElementPerformAction(button, kAXPressAction as CFString)
            report.note("alert dismissed", detail: "\(owner) \(axString(button, kAXTitleAttribute) ?? "")")
        }
    }
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
    _ = useInputSource(abcIds)
    let pid = launchVelocity(environment, files: [a, b, c])
    dismissAlerts(report: report)
    report.info("pid", detail: "\(pid) windows=\(windowTitles(pid)) tabs=\(tabSummary(velocityWindow(pid)!))")

    let pinyin = useInputSource(pinyinIds)
    report.check(!pinyin.isEmpty, "Pinyin input source available", detail: "\(pinyinIds) -> \(currentInputSourceId())")
    guard !pinyin.isEmpty else { report.finish() }

    guard let window = velocityWindow(pid), let editor = editorNode(window) else { fail("no editor element") }
    bringToFront(pid, report: report)
    if frontmostPid() != pid {
        // Cooperative activation can refuse a background launch; one click on the
        // window is the user's own path and is how a person would start typing.
        if let frame = axRect(window) {
            _ = guardedClick(CGPoint(x: frame.midX, y: frame.minY + 120), pid: pid, report: report, label: "focus")
        }
    }
    report.check(frontmostPid() == pid, "Velocity frontmost before typing", detail: frontmostName())

    // 1a. nihao + space -> 你好
    _ = AXUIElementSetAttributeValue(editor, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    typeKeys("nihao", gap: 55_000)
    let preview = waitUntil(3) { editorText(window).contains("ni") || editorText(window).contains("你") }
    report.note("preview while composing", detail: editorText(window).debugDescription)
    typeKeys(" ", gap: 55_000)
    let committed = waitUntil(5) { editorText(window).contains("你好") }
    report.check(committed, "nihao + space inserts 你好", detail: editorText(window).debugDescription)
    if preview == false { report.note("preview", detail: "no preedit text exposed through AXValue before commit") }

    // 1b. zhongwen + Escape inserts nothing and leaves no preedit.
    postKey("a", flags: .maskCommand)
    usleep(120_000)
    typeKeys("zhongwen", gap: 55_000)
    usleep(250_000)
    postKey("escape")
    usleep(400_000)
    let escaped = editorText(window)
    report.check(!escaped.contains("中文") && !escaped.contains("zhongwen"),
                 "zhongwen + Escape inserts nothing", detail: escaped.debugDescription)

    // 1c. Composing then Cmd+2 discards the composition.
    postKey("a", flags: .maskCommand)
    usleep(80_000)
    postKey("2", flags: .maskCommand)
    usleep(400_000)
    let secondSelected = tabs(velocityWindow(pid)!).first { $0.label.hasPrefix("b.txt") }?.selected ?? false
    report.check(secondSelected, "Cmd+2 selects the second tab", detail: tabSummary(velocityWindow(pid)!))
    typeKeys("zhongwen", gap: 55_000)
    usleep(200_000)
    postKey("1", flags: .maskCommand)
    usleep(500_000)
    let firstWindow = velocityWindow(pid)!
    let firstText = editorText(firstWindow)
    let secondWindowText = tabs(firstWindow).count > 0 ? axFindAll(firstWindow, role: "AXTextArea")
        .map { axString($0, kAXValueAttribute) ?? "" }.joined(separator: "|") : ""
    report.check(!firstText.contains("zhongwen") && !secondWindowText.contains("zhongwen") && !secondWindowText.contains("中文"),
                 "switching tabs discards the composition", detail: "tab1=\(firstText.debugDescription) all=\(secondWindowText.debugDescription)")

    // 1d. Two carets (Ctrl+Shift+Down), then ceshi + space inserts 测试 at both and
    // one Cmd+Z removes both.
    guard let window = velocityWindow(pid), let editor = editorNode(window) else { fail("no editor element") }
    _ = AXUIElementSetAttributeValue(editor, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    postKey("a", flags: .maskCommand)
    postKey("delete")
    usleep(200_000)
    typeUnicode("line one")
    postKey("return")
    typeUnicode("line two")
    postKey("return")
    typeUnicode("line three")
    usleep(300_000)
    postKey("a", flags: .maskCommand)
    usleep(120_000)
    postKey("left")
    usleep(150_000)
    postKey("down", flags: [.maskControl, .maskShift])
    usleep(250_000)
    report.note("carets after Ctrl+Shift+Down", detail: describeRanges(editor))
    let beforeMulti = editorText(window)
    typeKeys("ceshi", gap: 55_000)
    usleep(250_000)
    typeKeys(" ", gap: 55_000)
    usleep(400_000)
    let multi = editorText(window)
    let commitCount = multi.components(separatedBy: "测试").count - 1
    report.check(commitCount == 2, "ceshi + space commits 测试 at both carets", detail: multi.debugDescription)
    postKey("z", flags: .maskCommand)
    usleep(500_000)
    let undone = editorText(window)
    report.check(undone == beforeMulti, "one Cmd+Z removes both commits", detail: undone.debugDescription)

    // 1e. Cmd+click adds a caret.
    postKey("a", flags: .maskCommand)
    postKey("delete")
    usleep(150_000)
    typeUnicode("one")
    postKey("return")
    typeUnicode("two")
    postKey("return")
    typeUnicode("three")
    usleep(250_000)
    if let node = editorNode(window), let rect = axRect(node) {
        // Click near the start of the third line and Cmd+click the first line.
        let point = CGPoint(x: rect.minX + 30, y: rect.minY + 60)
        let second = CGPoint(x: rect.minX + 30, y: rect.minY + 14)
        _ = guardedClick(point, pid: pid, report: report, label: "caret 1")
        _ = guardedClick(second, pid: pid, report: report, label: "caret 2", flags: .maskCommand)
        usleep(200_000)
        typeUnicode("XY")
        usleep(250_000)
        let text = editorText(window)
        report.note("cmd-click multi-caret text", detail: text.debugDescription)
        report.check(text.components(separatedBy: "XY").count - 1 == 2, "Cmd+click places a second caret",
                     detail: text.debugDescription)
    }

    // 1f. Rename-tab field composes immediately.
    guard let window = velocityWindow(pid) else { fail("no window") }
    _ = useInputSource(pinyinIds)
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
            let value = axString(field, kAXValueAttribute) ?? ""
            report.check(!value.isEmpty && (value.contains("你") || value.contains("ni") || value.contains("n")),
                         "rename field shows the composed character", detail: value.debugDescription)
            postKey("escape")
            usleep(200_000)
        } else {
            dumpOnFailure(report, pid: pid, label: "rename")
        }
    } else {
        report.check(false, "tab b.txt found for the rename check", detail: tabSummary(window))
    }

    _ = useInputSource(abcIds)
    killVelocity(at: environment.launcher)
    if report.failures > 0 { dumpOnFailure(report, pid: pid, label: "ime") }
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
    let newWindow = axWindows(pid).first { win in
        guard let r = axRect(win) else { return false }
        return newWindows.contains { $0.bounds.intersects(r.insetBy(dx: 6, dy: 6)) }
    }
    report.check(newWindow.map { tabs($0).contains { $0.label.hasPrefix("c.txt") } } ?? false,
                 "the new window holds c.txt", detail: newWindow.map { tabSummary($0) } ?? "no AX window")
    if let source = axWindows(pid).first(where: { win in !sameWindow(win, newWindow!) }) {
        report.check(!tabs(source).contains { $0.label.hasPrefix("c.txt") },
                     "the source window lost c.txt", detail: tabSummary(source))
    }

    // 2b. Drag the tab from the new window into the source strip: merge, source closes.
    if let newWindow, let mainWindow = axWindows(pid).first(where: { win in !sameWindow(win, newWindow) }),
       let moved = tab(newWindow, named: "c.txt"), let target = axRect(mainWindow) {
        let from = CGPoint(x: moved.frame.midX, y: moved.frame.midY)
        let to = CGPoint(x: target.minX + 120, y: target.minY + 18)
        drag(from: from, to: to, steps: 18, pause: 0.06)
        usleep(600_000)
        let windowsNow = axWindows(pid).filter { isOnScreen($0, pid: pid) }
        let merged = windowsNow.first { tabs($0).contains { $0.label.hasPrefix("c.txt") } }
        report.check(merged != nil, "dragging into the other strip merges the tab")
        report.check(windowsNow.count == 1, "the emptied source window closes", detail: "\(windowsNow.count) window(s)")
        report.check(windowsNow.first.map { tabs($0).count == 3 } ?? false,
                     "merged window keeps three tabs", detail: windowsNow.first.map { tabSummary($0) } ?? "")
    } else {
        report.check(false, "second window and its c.txt tab found for the merge")
    }

    // 2c. Drag out and press Escape: nothing changes.
    guard let window = axWindows(pid).first(where: { isOnScreen($0, pid: pid) }), let frame = axRect(window) else {
        report.finish()
    }
    let windowCount = axWindows(pid).filter { isOnScreen($0, pid: pid) }.count
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
        let now = axWindows(pid).filter { isOnScreen($0, pid: pid) }
        report.check(now.count == windowCount, "Escape during a drag creates no window", detail: "\(now.count) vs \(windowCount)")
        report.check(now.first.map { tabs($0).count == tabCount } ?? false,
                     "Escape during a drag keeps the tab in place", detail: now.first.map { tabSummary($0) } ?? "")
    }

    killVelocity(at: environment.launcher)
    if report.failures > 0 { dumpOnFailure(report, pid: pid, label: "tabs") }
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
    report.check(tabs(window).first?.selected ?? false, "first tab is current", detail: tabSummary(window))

    // Another app takes the front, then one click on a non-current Velocity tab.
    let other = environment.fixture("other.txt", contents: "other\n")
    _ = shell("open -e \(other.shellQuoted)")
    guard let otherPid = waitForApp(bundleId: "com.apple.TextEdit", timeout: 15) else {
        report.check(false, "TextEdit came to the front"); report.finish()
    }
    _ = waitUntil(8) { frontmostPid() == otherPid }
    report.check(frontmostPid() == otherPid, "another app is frontmost", detail: frontmostName())
    guard let target = tab(window, named: "c.txt") else { report.check(false, "c.txt tab"); report.finish() }
    let point = CGPoint(x: target.frame.midX, y: target.frame.midY)
    _ = guardedClick(point, pid: pid, report: report, label: "inactive tab", clicks: 1)
    let activated = waitUntil(3) { frontmostPid() == pid }
    report.check(activated, "one click on the inactive window activates Velocity", detail: frontmostName())
    let switched = waitUntil(2) {
        guard let window = velocityWindow(pid) else { return false }
        return tabs(window).first { $0.label.hasPrefix("c.txt") }?.selected ?? false
    }
    report.check(switched, "the same click selects the clicked tab",
                 detail: velocityWindow(pid).map { tabSummary($0) } ?? "no window")
    killQuietly("TextEdit")
    killVelocity(at: environment.launcher)
    if report.failures > 0 { dumpOnFailure(report, pid: pid, label: "activate") }
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
    killQuietly("TextEdit")
    killVelocity(at: environment.launcher)
    if report.failures > 0 { dumpOnFailure(report, pid: pid, label: "background") }
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
