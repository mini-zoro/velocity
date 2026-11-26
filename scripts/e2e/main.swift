// GUI end-to-end driver for Velocity: probe and scenario subcommands.
//
// Invoked by scripts/e2e/run.sh; every command reads and writes the real screen
// session through HID events and the accessibility tree. Commands never require
// Accessibility permission to *print* a result: a refused AX read is reported.
import AppKit
import Carbon.HIToolbox
import IOKit.pwr_mgt
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation

_ = NSApplication.shared
NSApp.setActivationPolicy(.prohibited)

let arguments = Array(CommandLine.arguments.dropFirst())
guard let command = arguments.first else {
    fail("usage: e2e <probe|session|ax|key|type|unicode|click|drag|windows|front|activate|inputs|select-input|launch|scenario|...>")
}
let options = Array(arguments.dropFirst())

// MARK: - Session

func sessionDictionary() -> [String: Any] {
    guard let raw = CGSessionCopyCurrentDictionary() as? [String: Any] else { return [:] }
    return raw
}

func consoleUser() -> String {
    output("stat -f %Su /dev/console") ?? "?"
}

func reportSession(_ report: Report) {
    let dictionary = sessionDictionary()
    report.info("console user", detail: consoleUser())
    report.info("GUI session", detail: dictionary.isEmpty ? "none" : "yes")
    report.info("on-console", detail: String(describing: dictionary["kCGSSessionOnConsoleKey"] ?? "?"))
    report.info("login-done", detail: String(describing: dictionary["kCGSessionLoginDoneKey"] ?? "?"))
    report.info("screen-locked", detail: String(describing: dictionary["CGSSessionScreenIsLocked"] ?? "false"))
    report.info("user name", detail: String(describing: dictionary["kCGSSessionUserNameKey"] ?? "?"))
    let screens = NSScreen.screens.map { "\($0.frame.width)x\($0.frame.height)@\($0.backingScaleFactor)x" }
    report.info("screens", detail: screens.joined(separator: ", ") + " main=\(NSScreen.main?.frame ?? .zero)")
    report.info("frontmost", detail: "\(frontmostName()) pid=\(frontmostPid())")
    report.info("pointer", detail: "\(Int(pointerLocation().x)),\(Int(pointerLocation().y))")
}

// MARK: - Probe

func probe() -> Never {
    let report = Report("probe")
    lockScreenOn()

    // 1. Machine, SIP and GUI session.
    report.info("os", detail: output("sw_vers -productVersion") ?? "?")
    report.info("host", detail: (output("uname -m") ?? "?") + " " + (output("id -un") ?? "?"))
    report.info("csrutil", detail: output("csrutil status").map { $0.replacingOccurrences(of: "\n", with: " ") } ?? "unavailable")
    report.info("sudo -n", detail: shell("sudo -n true; echo $? ").out)
    report.info("system_profiler GPU", detail: output("system_profiler SPDisplaysDataType 2>/dev/null | grep -E 'Chipset Model|Metal|Vendor|Resolution|VRAM'") ?? "none")
    reportSession(report)

    // 2. Screen capture with the legacy API (screencapture, as a bash child).
    let shot = "/tmp/e2e-probe-shot.png"
    try? FileManager.default.removeItem(atPath: shot)
    let capture = shell("screencapture -x \(shot) 2>&1; echo status=$?")
    let exists = FileManager.default.fileExists(atPath: shot)
    let size = exists ? (output("sips -g pixelWidth -g pixelHeight \(shot) | tail -2 | tr '\\n' ' '") ?? "?") : "missing"
    report.check(exists, "screencapture of the screen", detail: "\(capture.out) \(size)")
    // CGWindowList names are only populated with Screen Recording permission.
    let named = windowList(layers: [0, 25]).filter { !$0.name.isEmpty }.count
    report.info("CGWindowList named windows", detail: "\(named) of \(windowList(layers: [0, 25]).count)")

    // 3. Accessibility trust for this helper and its ancestors.
    let trusted = AXIsProcessTrusted()
    report.check(trusted, "AXIsProcessTrusted() for the swift helper", detail: String(describing: ProcessInfo.processInfo.processIdentifier))
    report.info("ppid chain", detail: parentChain())
    if let finder = pid_t(output("pgrep -x Finder") ?? "0") {
        var raw: AnyObject?
        let status = AXUIElementCopyAttributeValue(axApp(finder), kAXWindowsAttribute as CFString, &raw)
        let count = (raw as? [AXUIElement])?.count ?? 0
        report.info("AX read of Finder windows", detail: "status=\(status.rawValue) windows=\(count) (refusal: -25211 API disabled, -25204 cannot complete)")
    }
    if trusted == false {
        // The fallback every hosted macOS UI test uses: a user-level TCC row for the
        // helper's absolute path. A fresh process (not this one) must see it.
        let granted = grantAccessibility(to: CommandLine.arguments[0])
        report.note("AX trust missing", detail: "grant attempt: \(granted)")
        let fresh = shell("\(CommandLine.arguments[0].shellQuoted) trust")
        report.info("AX trust in a fresh process", detail: fresh.out)
    }

    // 4. TCC databases: can a root or user entry be added?
    let systemDb = "/Library/Application Support/com.apple.TCC/TCC.db"
    let userDb = "\(NSHomeDirectory())/Library/Application Support/com.apple.TCC/TCC.db"
    report.info("system TCC.db", detail: shell("sqlite3 '\(systemDb)' \"select service, client, auth_value from access where service in ('kTCCServiceAccessibility','kTCCServicePostEvent','kTCCServiceScreenCapture') and (client like '%bash%' or client like '%runner%' or client like '%hca%' or client like '%swift%') limit 20\" 2>&1").out)
    report.info("user TCC.db", detail: shell("sqlite3 '\(userDb)' \"select service, client, auth_value from access where service in ('kTCCServiceAccessibility','kTCCServicePostEvent') limit 20\" 2>&1").out)
    let writeTest = shell("sqlite3 '\(userDb)' \"insert or ignore into access (service, client, client_type, auth_value, auth_reason, auth_version, flags) values ('kTCCServiceProbe', '$0', 1, 2, 0, 1, 0); select changes();\" 2>&1")
    report.info("user TCC.db write", detail: writeTest.out)
    _ = shell("sqlite3 '\(userDb)' \"delete from access where service='kTCCServiceProbe'\" 2>&1")
    let systemWrite = shell("sudo -n sqlite3 '\(systemDb)' \"select count(*) from access\" 2>&1")
    report.info("system TCC.db via sudo", detail: systemWrite.out)

    // 5. HID keyboard: a self-owned window first (no IME involvement).
    probeSelfTyping(report)
    // 6. HID keyboard into another app (TextEdit).
    let textEdit = probeTextEdit(report)
    // 7. HID mouse into another app.
    if let textEdit { probeMouse(report, textEdit: textEdit) }

    // 8. Input sources and the Pinyin IME.
    let sourceList = inputSources()
    let pinyin = sourceList.filter { stringProperty($0, kTISPropertyInputSourceID as String).contains("SCIM") }
        .map { stringProperty($0, kTISPropertyInputSourceID as String) }
    report.info("SCIM input sources", detail: pinyin.joined(separator: ", "))
    report.info("enabled layouts", detail: sourceList.filter { boolProperty($0, kTISPropertyInputSourceIsEnabled as String) }
        .filter { stringProperty($0, kTISPropertyInputSourceCategory as String).contains("Keyboard") }
        .map { stringProperty($0, kTISPropertyInputSourceID as String) }.joined(separator: ", "))
    let pinyinCurrent = switchToPinyin(report)
    report.check(pinyinCurrent, "Pinyin is the current input source", detail: currentInputSourceId())
    if pinyinCurrent, let textEdit {
        let area = axFind(axApp(textEdit), role: "AXTextArea")
        if let area { _ = AXUIElementSetAttributeValue(area, kAXFocusedAttribute as CFString, kCFBooleanTrue) }
        selectAll(pid: textEdit)
        typeKeys("nihao", gap: 60_000)
        usleep(600_000)
        typeKeys(" ", gap: 60_000)
        let committed = waitUntil(4) { readText(pid: textEdit).contains("你好") }
        report.check(committed, "Pinyin HID: nihao + space yields 你好", detail: "value=\(readText(pid: textEdit).debugDescription)")
        if !committed {
            try? axDumpFile(pid: textEdit, to: "/tmp/e2e-probe-textedit.txt")
            report.note("TextEdit AX dump", detail: "/tmp/e2e-probe-textedit.txt")
        }
        typeKeys("zhongwen", gap: 60_000)
        usleep(400_000)
        postKey("escape")
        usleep(300_000)
        let afterEscape = readText(pid: textEdit)
        report.check(!afterEscape.contains("中文") && !afterEscape.contains("zhongwen"), "Pinyin HID: zhongwen + Escape inserts nothing",
                     detail: "value=\(afterEscape.debugDescription)")
    }
    // Restore the ABC layout so later checks can type plainly.
    let abc = useInputSource(abcIds)
    report.info("restored input source", detail: abc.isEmpty ? "none of \(abcIds) available" : abc)

    // 9. Metal: the GPU tests ask wgpu for the real device.
    report.note("Metal", detail: "the workflow runs vel-render memory_tests with --nocapture; see the GPU adapter line there")

    // 10. Process footprint availability.
    report.info("footprint", detail: footprintBytes(getpid()).map { "\($0 / 1_048_576) MiB for the helper" } ?? "unavailable")
    report.finish()
}

/// Keeps the screen awake for the whole job (the display sleeping breaks every HID
/// path exactly like a locked screen).
func lockScreenOn() {
    var assertion: IOPMAssertionID = 0
    _ = IOPMAssertionCreateWithName("NoDisplaySleepAssertion" as CFString, IOPMAssertionLevel(kIOPMAssertionLevelOn),
                                    "velocity gui e2e" as CFString, &assertion)
}

func parentChain() -> String {
    var chain: [String] = []
    var pid = getppid()
    for _ in 0..<8 where pid > 1 {
        let exe = output("ps -o comm= -p \(pid)") ?? "?"
        chain.append("\(pid):\(exe)")
        if let raw = output("ps -o ppid= -p \(pid)")?.trimmingCharacters(in: .whitespaces), let next = pid_t(raw) {
            pid = next
        } else { break }
    }
    return chain.joined(separator: " <- ")
}

func readText(pid: pid_t) -> String {
    guard let area = axFind(axApp(pid), role: "AXTextArea") else { return "" }
    return axString(area, kAXValueAttribute) ?? ""
}

func selectAll(pid: pid_t) {
    postKey("a", flags: .maskCommand)
    usleep(80_000)
}

func probeSelfTyping(_ report: Report) {
    // A tool window needs at least the accessory policy to hold the keyboard.
    NSApp.setActivationPolicy(.accessory)
    let window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 420, height: 160),
                          styleMask: [.titled], backing: .buffered, defer: false)
    window.title = "Velocity e2e self-typing probe"
    let field = NSTextField(frame: NSRect(x: 10, y: 10, width: 400, height: 120))
    field.isEditable = true
    window.contentView?.addSubview(field)
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
    window.makeFirstResponder(field)
    usleep(300_000)
    let front = frontmostPid() == getpid()
    NSApp.activate(ignoringOtherApps: true)
    let policy = NSRunningApplication.current.activationPolicy
    report.info("self-typing setup", detail: "frontmost=self:\(front) policy=\(policy.rawValue) front=\(frontmostName())")
    // A Unicode event and a layout key event exercise different event paths.
    // NSApp.run() is not used here, so dispatch the queued events by hand.
    typeUnicode("xyz")
    typeKeys("abc")
    pumpEvents(seconds: 0.6)
    report.check(field.stringValue.contains("abc"), "HID keyboard into the helper's own window", detail: "value=\(field.stringValue.debugDescription)")
    window.orderOut(nil)
}

func probeTextEdit(_ report: Report) -> pid_t? {
    let path = "/tmp/velocity-e2e-probe.txt"
    try? "".write(toFile: path, atomically: true, encoding: .utf8)
    let (status, out) = shell("open -e \(path) 2>&1; echo status=$?")
    guard let pid = waitForApp(bundleId: "com.apple.TextEdit", timeout: 15) else {
        report.check(false, "TextEdit launched", detail: out)
        return nil
    }
    _ = status
    _ = waitUntil(10) { !axWindows(pid).isEmpty }
    NSRunningApplication(processIdentifier: pid)?.activate(options: [.activateAllWindows])
    usleep(800_000)
    let front = frontmostPid()
    report.check(front == pid, "TextEdit frontmost after launch", detail: "front=\(frontmostName()) pid=\(front)")
    if let area = axFind(axApp(pid), role: "AXTextArea") {
        _ = AXUIElementSetAttributeValue(area, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    }
    selectAll(pid: pid)
    typeKeys("abc def")
    // TextEdit capitalizes the first word of a paragraph; the keys are what matter.
    let typed = waitUntil(4) { readText(pid: pid).lowercased().contains("abc def") }
    report.check(typed, "HID keyboard into TextEdit", detail: "value=\(readText(pid: pid).debugDescription)")
    if !typed {
        try? axDumpFile(pid: pid, to: "/tmp/e2e-probe-textedit.txt")
        report.note("TextEdit AX dump", detail: "/tmp/e2e-probe-textedit.txt")
    }
    return pid
}

func probeMouse(_ report: Report, textEdit pid: pid_t) {
    guard let window = axWindows(pid).first, let frame = axRect(window) else {
        report.check(false, "TextEdit window frame")
        return
    }
    selectAll(pid: pid)
    typeKeys("0123456789")
    usleep(200_000)
    // Click just right of the first glyph line start: the caret should move there.
    let left = CGPoint(x: frame.minX + 24, y: frame.minY + 40)
    click(at: left)
    usleep(200_000)
    typeKeys("L")
    usleep(250_000)
    let leftValue = readText(pid: pid)
    let caretIndex = leftValue.firstIndex(of: "L").map { leftValue.distance(from: leftValue.startIndex, to: $0) } ?? -1
    report.check(caretIndex >= 0 && caretIndex <= 6, "HID click places the caret in TextEdit",
                 detail: "caret=\(caretIndex) value=\(leftValue.debugDescription)")
    // Double click selects a word; typing replaces it.
    click(at: CGPoint(x: frame.minX + 60, y: frame.minY + 40), clicks: 2)
    usleep(200_000)
    typeKeys("Z")
    usleep(250_000)
    report.check(readText(pid: pid).count < leftValue.count, "HID double click selects a word", detail: "value=\(readText(pid: pid).debugDescription)")
}

func waitForApp(bundleId: String, timeout: Double) -> pid_t? {
    var found: pid_t?
    _ = waitUntil(timeout, interval: 0.2) {
        found = NSRunningApplication.runningApplications(withBundleIdentifier: bundleId).first?.processIdentifier
        return found != nil
    }
    return found
}

/// Delivers queued NSEvents without running the app's main loop.
func pumpEvents(seconds: Double) {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        guard let event = NSApp.nextEvent(matching: .any, until: Date().addingTimeInterval(0.02),
                                          inMode: .default, dequeue: true) else { continue }
        NSApp.sendEvent(event)
    }
}

func axDumpFile(pid: pid_t, to path: String) throws {
    var lines: [String] = []
    axDump(axApp(pid), into: &lines)
    try lines.joined(separator: "\n").write(toFile: path, atomically: true, encoding: .utf8)
}

// MARK: - TCC fallback

/// Adds Accessibility and PostEvent rows for `path`, first in the user database
/// and then (when passwordless sudo is available) in the system database. Returns
/// one status per service and database; the caller re-checks from a fresh process.
func grantAccessibility(to path: String) -> String {
    let resolved = URL(fileURLWithPath: path).resolvingSymlinksInPath().path
    let databases = [
        ("user", "", "\(NSHomeDirectory())/Library/Application Support/com.apple.TCC/TCC.db"),
        ("system", "sudo -n ", "/Library/Application Support/com.apple.TCC/TCC.db"),
    ]
    var results: [String] = []
    for (scope, prefix, db) in databases {
        for service in ["kTCCServiceAccessibility", "kTCCServicePostEvent"] {
            let sql = "insert or replace into access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags) values ('\(service)', '\(resolved)', 1, 2, 0, 1, NULL, NULL, NULL, 'UNUSED', NULL, 0);"
            let (status, out) = shell("\(prefix)sqlite3 \(db.shellQuoted) \(sql.shellQuoted) 2>&1")
            results.append("\(scope)/\(service)=\(status)\(out.isEmpty ? "" : " (\(out))")")
        }
    }
    return results.joined(separator: ", ")
}

// MARK: - Subcommands

switch command {
case "probe":
    probe()
case "switch-ime":
    switchInputMethodToPinyin()
case "move-window":  // move-window <pid> <title-substring> <x> <y> <w> <h>
    guard options.count >= 6, let pid = pid_t(options[0]) else { fail("move-window <pid> <title> x y w h") }
    guard let window = velocityWindow(pid, title: options[1]) else { fail("window \(options[1]) not found") }
    let rect = CGRect(x: Double(options[2]) ?? 0, y: Double(options[3]) ?? 0,
                      width: Double(options[4]) ?? 800, height: Double(options[5]) ?? 600)
    print("moved=\(moveWindow(window, to: rect))")
case "ime-info":  // read-only inspection of installed input sources
    for id in (options.isEmpty ? ["com.apple.inputmethod.SCIM"] + pinyinIds : options) {
        print(inputSourceInfo(id))
    }
    print("current=\(currentInputSourceId())")
case "trust":
    print("ax_trusted=\(AXIsProcessTrusted())")
case "grant":  // grant <path>
    print(grantAccessibility(to: options.first ?? CommandLine.arguments[0]))
case "session":
    let report = Report("session")
    reportSession(report)
    report.finish()
case "ax":  // ax <pid> [max-depth] [file]
    guard let pid = pid_t(options.first ?? "") else { fail("ax needs a pid") }
    let depth = options.count > 1 ? Int(options[1]) ?? 12 : 12
    var lines: [String] = []
    axDump(axApp(pid), maxDepth: depth, into: &lines)
    let text = lines.joined(separator: "\n")
    if options.count > 2 { try? text.write(toFile: options[2], atomically: true, encoding: .utf8) }
    print(text)
case "key":  // key <name> [modifiers]
    postKey(options.first ?? "", flags: modifierFlags(options.count > 1 ? options[1] : ""))
case "type":  // type <text>
    typeKeys(options.joined(separator: options.count > 1 ? " " : ""))
case "unicode":
    typeUnicode(options.joined(separator: " "))
case "click":  // click <x> <y> [clicks] [modifiers]
    guard options.count >= 2 else { fail("click needs x y") }
    click(at: CGPoint(x: Double(options[0]) ?? 0, y: Double(options[1]) ?? 0),
          clicks: options.count > 2 ? Int(options[2]) ?? 1 : 1,
          flags: modifierFlags(options.count > 3 ? options[3] : ""))
case "drag":  // drag <x1> <y1> <x2> <y2> [steps]
    guard options.count >= 4 else { fail("drag needs x1 y1 x2 y2") }
    drag(from: CGPoint(x: Double(options[0]) ?? 0, y: Double(options[1]) ?? 0),
         to: CGPoint(x: Double(options[2]) ?? 0, y: Double(options[3]) ?? 0),
         steps: options.count > 4 ? Int(options[4]) ?? 12 : 12)
case "windows":  // windows [owner]
    for window in windowList(owner: options.first, layers: [0, 8, 25, 101]) {
        print("id=\(window.id) pid=\(window.pid) owner=\(window.owner) layer=\(window.layer) name=\(window.name) bounds=\(Int(window.bounds.minX)),\(Int(window.bounds.minY)) \(Int(window.bounds.width))x\(Int(window.bounds.height))")
    }
case "front":
    print("\(frontmostPid()) \(frontmostName())")
case "activate":
    guard let pid = pid_t(options.first ?? "") else { fail("activate needs a pid") }
    print("activate=\(activate(pid)) front=\(frontmostPid() == pid)")
case "inputs":
    for source in inputSources() {
        let id = stringProperty(source, kTISPropertyInputSourceID as String)
        if let filter = options.first, !id.contains(filter) { continue }
        print("\(id) enabled=\(boolProperty(source, kTISPropertyInputSourceIsEnabled as String)) selectable=\(boolProperty(source, kTISPropertyInputSourceIsSelectCapable as String)) category=\(stringProperty(source, kTISPropertyInputSourceCategory as String))")
    }
    print("current=\(currentInputSourceId())")
case "select-input":
    guard let id = options.first else { fail("select-input needs an id") }
    let enable = enableInputSource(id)
    let select = selectInputSource(id)
    _ = waitUntil(3) { currentInputSourceId() == id }
    print("enable=\(enable) select=\(select) current=\(currentInputSourceId())")
case "scenario":
    guard let name = options.first else { fail("scenario needs a name") }
    switch name {
    case "ime": scenarioIME(options.dropFirst())
    case "tabs": scenarioTabs(options.dropFirst())
    case "activate": scenarioActivate(options.dropFirst())
    case "background": scenarioBackground(options.dropFirst())
    case "footprint": scenarioFootprint(options.dropFirst())
    default: fail("unknown scenario \(name)")
    }
default:
    fail("unknown command \(command)")
}
