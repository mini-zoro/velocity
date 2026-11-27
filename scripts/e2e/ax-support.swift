// Shared support for the GUI end-to-end harness (scripts/e2e).
//
// Everything here posts real HID events (CGEventPost to .cghidEventTap) and reads
// the public accessibility tree, so it exercises the same path a person's
// keyboard, mouse and input method use. It never needs to be inside Velocity.
import AppKit
import ApplicationServices
import Carbon.HIToolbox
import CoreGraphics
import Foundation

// MARK: - Reporting

final class Report: @unchecked Sendable {
    private(set) var failures = 0
    private(set) var passes = 0
    private var lines: [String] = []
    private let started = Date()
    let name: String

    init(_ name: String) {
        self.name = name
    }

    /// Records a verdict. AX reads that failed since the previous check (see
    /// `AXReadErrors`) are reported as their own FAIL right before it: a verdict
    /// built on a tree the harness could not read is not evidence either way.
    func check(_ ok: Bool, _ label: String, detail: String = "") {
        let errors = AXReadErrors.shared.drain()
        if !errors.isEmpty {
            failures += 1
            emit("FAIL", "AX reads failed before \"\(label)\"", summarizeAXErrors(errors))
        }
        if ok {
            passes += 1
            emit("PASS", label, detail)
        } else {
            failures += 1
            emit("FAIL", label, detail)
        }
    }

    func info(_ label: String, detail: String = "") {
        emit("INFO", label, detail)
    }

    func note(_ label: String, detail: String = "") {
        emit("NOTE", label, detail)
    }

    private func emit(_ kind: String, _ label: String, _ detail: String) {
        let line = "\(kind) [\(name)] \(label)\(detail.isEmpty ? "" : ": \(detail)")"
        lines.append(line)
        print(line)
        fflush(stdout)
    }

    /// Summary line every scenario ends with; the workflow greps it.
    func finish() -> Never {
        // Reads after the last verdict (cleanup, dumps) decided nothing; show them.
        let trailing = AXReadErrors.shared.drain()
        if !trailing.isEmpty { note("AX reads failed after the last check", detail: summarizeAXErrors(trailing)) }
        let seconds = String(format: "%.1f", Date().timeIntervalSince(started))
        let verdict = failures == 0 ? "PASS" : "FAIL"
        let line = "SUMMARY [\(name)] \(verdict) \(passes) passed, \(failures) failed, \(seconds)s"
        print(line)
        fflush(stdout)
        if let path = ProcessInfo.processInfo.environment["E2E_SUMMARY"] {
            let text = (lines + [line]).joined(separator: "\n") + "\n"
            try? text.write(toFile: path, atomically: true, encoding: .utf8)
        }
        exit(failures == 0 ? 0 : 1)
    }
}

func fail(_ message: String) -> Never {
    print("ERROR \(message)")
    fflush(stdout)
    exit(2)
}

func log(_ message: String) {
    print("LOG \(message)")
    fflush(stdout)
}

// MARK: - Shell helpers

@discardableResult
func shell(_ command: String) -> (status: Int32, out: String) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/zsh")
    process.arguments = ["-c", command]
    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = pipe
    do {
        try process.run()
    } catch {
        return (-1, "spawn failed: \(error)")
    }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    return (process.terminationStatus, String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines))
}

/// Runs a command and returns its trimmed output, or nil when it failed.
func output(_ command: String) -> String? {
    let (status, text) = shell(command)
    return status == 0 ? text : nil
}

// MARK: - Accessibility

/// AX reads that failed. A refused or timed-out call is not "no children" or "no
/// text": every such read is recorded here, and the next `Report.check` turns it into
/// a FAIL line, so a negative assertion can never pass on a tree that could not be
/// read. `attributeUnsupported` and `noValue` are answers, not failures; they are
/// not recorded. `waitUntil` drops errors from polls that were retried, keeping only
/// those of the poll that decided the outcome.
final class AXReadErrors: @unchecked Sendable {
    static let shared = AXReadErrors()
    private var pending: [String] = []
    private var quietDepth = 0

    static let benign: Set<Int32> = [AXError.attributeUnsupported.rawValue, AXError.noValue.rawValue,
                                     AXError.parameterizedAttributeUnsupported.rawValue]

    func record(_ attribute: String, _ code: AXError) {
        guard quietDepth == 0, code != .success, !AXReadErrors.benign.contains(code.rawValue) else { return }
        pending.append("\(attribute) \(axErrorName(code))")
    }

    func record(_ message: String) {
        guard quietDepth == 0 else { return }
        pending.append(message)
    }

    var mark: Int { pending.count }

    /// Removes and returns the errors recorded after `mark`.
    func rollback(to mark: Int) -> [String] {
        guard pending.count > mark else { return [] }
        let dropped = Array(pending[mark...])
        pending.removeSubrange(mark...)
        return dropped
    }

    func drain() -> [String] { rollback(to: 0) }

    /// Diagnostics (dumps, descriptions in details, other apps' alert sweeps) never
    /// change a verdict.
    func quietly<T>(_ body: () -> T) -> T {
        quietDepth += 1
        defer { quietDepth -= 1 }
        return body()
    }
}

private let axErrorNames: [Int32: String] = [
    AXError.failure.rawValue: "failure", AXError.illegalArgument.rawValue: "illegalArgument",
    AXError.invalidUIElement.rawValue: "invalidUIElement", AXError.cannotComplete.rawValue: "cannotComplete",
    AXError.attributeUnsupported.rawValue: "attributeUnsupported", AXError.notImplemented.rawValue: "notImplemented",
    AXError.apiDisabled.rawValue: "apiDisabled", AXError.noValue.rawValue: "noValue",
    AXError.parameterizedAttributeUnsupported.rawValue: "parameterizedAttributeUnsupported",
]

func axErrorName(_ code: AXError) -> String {
    "\(axErrorNames[code.rawValue] ?? "AXError")(\(code.rawValue))"
}

/// "AXChildren cannotComplete(-25204)×3, AXValue invalidUIElement(-25202)" in first-seen order.
func summarizeAXErrors(_ errors: [String]) -> String {
    var order: [String] = []
    var counts: [String: Int] = [:]
    for error in errors {
        if counts[error] == nil { order.append(error) }
        counts[error, default: 0] += 1
    }
    return order.map { counts[$0]! > 1 ? "\($0)×\(counts[$0]!)" : $0 }.joined(separator: ", ")
}

/// One read with its error code (benign codes included), recording nothing.
func axCopy(_ element: AXUIElement, _ name: String) -> (value: AnyObject?, error: AXError) {
    var value: AnyObject?
    let error = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    return (error == .success ? value : nil, error)
}

func axAttr(_ element: AXUIElement, _ name: String) -> AnyObject? {
    let (value, error) = axCopy(element, name)
    AXReadErrors.shared.record(name, error)
    return value
}

func axString(_ element: AXUIElement, _ name: String) -> String? {
    guard let value = axAttr(element, name) else { return nil }
    if let text = value as? String { return text }
    if let number = value as? NSNumber { return number.stringValue }
    return nil
}

/// A string attribute that must be there: nil plus the reason when it is not
/// (including `noValue`/`attributeUnsupported`, which `axString` treats as answers).
func axRequiredString(_ element: AXUIElement, _ name: String) -> (text: String?, why: String) {
    let (value, error) = axCopy(element, name)
    guard error == .success else {
        AXReadErrors.shared.record(name, error)
        return (nil, "\(name) \(axErrorName(error))")
    }
    guard let text = value as? String else { return (nil, "\(name) is not a string") }
    return (text, "")
}

func axBool(_ element: AXUIElement, _ name: String) -> Bool? {
    (axAttr(element, name) as? NSNumber)?.boolValue
}

/// An array-of-elements attribute; a failed read is recorded, never just "empty".
func axElements(_ element: AXUIElement, _ name: String) -> [AXUIElement] {
    guard let value = axAttr(element, name) else { return [] }
    guard CFGetTypeID(value) == CFArrayGetTypeID(), let elements = value as? [AXUIElement] else {
        AXReadErrors.shared.record("\(name) returned a non-array")
        return []
    }
    return elements
}

func axChildren(_ element: AXUIElement) -> [AXUIElement] {
    axElements(element, kAXChildrenAttribute)
}

func axElement(_ element: AXUIElement, _ name: String) -> AXUIElement? {
    guard let value = axAttr(element, name), CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return (value as! AXUIElement)
}

/// The AX client API hands ranges over as AXValue (kAXValueCFRangeType), never as
/// NSValue; casting to [NSValue] silently yields nothing.
func axRangeValue(_ value: AnyObject) -> NSRange? {
    guard CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var range = CFRange()
    guard AXValueGetValue(value as! AXValue, .cfRange, &range) else { return nil }
    return NSRange(location: range.location, length: range.length)
}

func axRange(_ element: AXUIElement, _ name: String) -> NSRange? {
    axAttr(element, name).flatMap(axRangeValue)
}

/// Every selected range (AXSelectedTextRanges), decoded from AXValue.
func axRanges(_ element: AXUIElement) -> [NSRange]? {
    guard let value = axAttr(element, kAXSelectedTextRangesAttribute), CFGetTypeID(value) == CFArrayGetTypeID() else { return nil }
    return (value as! [AnyObject]).compactMap(axRangeValue)
}

/// Screen rectangle (top-left origin, like AXFrame) of a text range.
func axBoundsForRange(_ element: AXUIElement, _ range: NSRange) -> CGRect? {
    var cf = CFRange(location: range.location, length: range.length)
    guard let parameter = AXValueCreate(.cfRange, &cf) else { return nil }
    var value: AnyObject?
    let error = AXUIElementCopyParameterizedAttributeValue(element, kAXBoundsForRangeParameterizedAttribute as CFString,
                                                          parameter, &value)
    guard error == .success else {
        AXReadErrors.shared.record(kAXBoundsForRangeParameterizedAttribute, error)
        return nil
    }
    guard let value, CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var rect = CGRect.zero
    guard AXValueGetValue(value as! AXValue, .cgRect, &rect) else { return nil }
    return rect
}

func axRect(_ element: AXUIElement) -> CGRect? {
    guard let value = axAttr(element, "AXFrame"), CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var rect = CGRect.zero
    guard AXValueGetValue(value as! AXValue, .cgRect, &rect) else { return nil }
    return rect
}

func axRole(_ element: AXUIElement) -> String {
    axString(element, kAXRoleAttribute) ?? ""
}

/// Depth-first search for the first element whose role equals `role` and whose
/// label matches `label` exactly (or by prefix when `prefix` is set).
func axFind(_ root: AXUIElement, role: String? = nil, label: String? = nil, prefix: Bool = false, parent: AXUIElement? = nil, depth: Int = 0) -> AXUIElement? {
    guard depth < 40 else { return nil }
    for child in axChildren(root) {
        let r = axRole(child)
        if r == "AXMenuBar" { continue }
        if role == nil || r == role {
            // Labels are read only when asked for: some apps (TextEdit) answer
            // AXDescription with kAXErrorFailure, which must not fail a role-only search.
            guard let label else { return child }
            let l = axString(child, kAXTitleAttribute) ?? axString(child, kAXDescriptionAttribute)
            if l == label || (prefix && (l?.hasPrefix(label) ?? false)) {
                return child
            }
            // AXTextField tab-rename field has a generic title; its description carries "Rename".
            if role == "AXTextField",
               (axString(child, kAXDescriptionAttribute) ?? axString(child, kAXHelpAttribute))?.contains(label) == true {
                return child
            }
        }
        if let found = axFind(child, role: role, label: label, prefix: prefix, parent: child, depth: depth + 1) {
            return found
        }
    }
    return nil
}

func axFindAll(_ root: AXUIElement, role: String, depth: Int = 0) -> [AXUIElement] {
    guard depth < 40 else { return [] }
    var found: [AXUIElement] = []
    for child in axChildren(root) where axRole(child) != "AXMenuBar" {
        if axRole(child) == role { found.append(child) }
        found.append(contentsOf: axFindAll(child, role: role, depth: depth + 1))
    }
    return found
}

func axApp(_ pid: pid_t) -> AXUIElement {
    AXUIElementCreateApplication(pid)
}

func axWindows(_ pid: pid_t) -> [AXUIElement] {
    axElements(axApp(pid), kAXWindowsAttribute)
}

/// The app's document windows (subrole AXStandardWindow), in AXWindows order.
/// AXWindows also lists windows the system creates inside the app: on macOS 14+,
/// typing through an input method shows an indicator bubble (a blue "A" near the
/// caret holding only `AXButton id=InputSource`), and it is the FIRST entry of
/// AXWindows until it fades a few seconds later. Never take AXWindows.first.
func axStandardWindows(_ pid: pid_t) -> [AXUIElement] {
    axWindows(pid).filter { axString($0, kAXSubroleAttribute) == kAXStandardWindowSubrole }
}

/// The window a person is working in: the app's main window when it is a standard
/// window, else its focused standard window, else its first standard window; only
/// when it has no standard window at all, whatever AXMainWindow names.
func axDocumentWindow(_ pid: pid_t) -> AXUIElement? {
    let app = axApp(pid)
    let standard = axStandardWindows(pid)
    let main = axElement(app, kAXMainWindowAttribute)
    for candidate in [main, axElement(app, kAXFocusedWindowAttribute)] {
        if let candidate, standard.contains(where: { CFEqual($0, candidate) }) { return candidate }
    }
    return standard.first ?? main
}

/// "AXStandardWindow "b.txt •", AXUnknown "" [AXButton id=InputSource]" — every
/// window the app owns, for notes about what AXWindows actually held.
func axWindowSummary(_ pid: pid_t) -> String {
    AXReadErrors.shared.quietly {
        axWindows(pid).map { window in
            let sub = axString(window, kAXSubroleAttribute) ?? "-"
            let inner = axChildren(window).prefix(3).map { child in
                "\(axRole(child))" + (axString(child, kAXIdentifierAttribute).map { " id=\($0)" } ?? "")
            }
            return "\(sub) \"\(axWindowTitle(window))\" [\(inner.joined(separator: ", "))]"
        }.joined(separator: "; ")
    }
}

func axWindowTitle(_ window: AXUIElement) -> String {
    axString(window, kAXTitleAttribute) ?? ""
}

/// Compact description of an element for logs and AX dumps. Diagnostic only: its
/// reads never record AX errors.
func axDescribe(_ element: AXUIElement) -> String {
    AXReadErrors.shared.quietly {
        var parts: [String] = ["role=\(axRole(element))"]
        for (key, attr) in [("sub", kAXSubroleAttribute), ("title", kAXTitleAttribute), ("desc", kAXDescriptionAttribute),
                            ("value", kAXValueAttribute), ("id", kAXIdentifierAttribute), ("help", kAXHelpAttribute)] {
            if let text = axString(element, attr) {
                var shown = text.replacingOccurrences(of: "\n", with: "⏎")
                if shown.count > 200 { shown = String(shown.prefix(200)) + "…(\(text.count))" }
                parts.append("\(key)=\(shown)")
            }
        }
        if let selected = axBool(element, kAXSelectedAttribute) { parts.append("sel=\(selected)") }
        if let focused = axBool(element, kAXFocusedAttribute), focused { parts.append("focus") }
        if let rect = axRect(element) { parts.append("frame=\(Int(rect.minX)),\(Int(rect.minY)) \(Int(rect.width))x\(Int(rect.height))") }
        return parts.joined(separator: " ")
    }
}

/// Places and sizes a window through the accessibility API (standard NSWindow
/// attributes), so two windows can be arranged without a title-bar drag.
@discardableResult
func moveWindow(_ window: AXUIElement, to rect: CGRect) -> Bool {
    var origin = CGPoint(x: rect.minX, y: rect.minY)
    var size = CGSize(width: rect.width, height: rect.height)
    let position = AXValueCreate(.cgPoint, &origin)!
    let extent = AXValueCreate(.cgSize, &size)!
    let moved = AXUIElementSetAttributeValue(window, kAXPositionAttribute as CFString, position)
    let sized = AXUIElementSetAttributeValue(window, kAXSizeAttribute as CFString, extent)
    return moved == .success && sized == .success
}

/// Diagnostic tree dump. A subtree that cannot be read says so with the error code
/// instead of looking like a leaf.
func axDump(_ element: AXUIElement, depth: Int = 0, maxDepth: Int = 12, into lines: inout [String]) {
    lines.append(String(repeating: "  ", count: depth) + axDescribe(element))
    guard depth < maxDepth else { return }
    let (value, error) = axCopy(element, kAXChildrenAttribute)
    if error != .success, !AXReadErrors.benign.contains(error.rawValue) {
        lines.append(String(repeating: "  ", count: depth + 1) + "<AXChildren \(axErrorName(error))>")
        return
    }
    for child in (value as? [AXUIElement]) ?? [] {
        if AXReadErrors.shared.quietly({ axRole(child) }) == "AXMenuBar" {
            lines.append(String(repeating: "  ", count: depth + 1) + "AXMenuBar (skipped)")
            continue
        }
        axDump(child, depth: depth + 1, maxDepth: maxDepth, into: &lines)
    }
}

// MARK: - Windows (CoreGraphics)

struct WindowInfo {
    let id: Int
    let pid: pid_t
    let owner: String
    let layer: Int
    let name: String
    let bounds: CGRect
}

func windowList(owner: String? = nil, layers: Set<Int> = [0]) -> [WindowInfo] {
    let raw = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]) ?? []
    return raw.compactMap { entry in
        guard let pid = entry[kCGWindowOwnerPID as String] as? pid_t,
              let layer = entry[kCGWindowLayer as String] as? Int,
              layers.contains(layer),
              let boundsDict = entry[kCGWindowBounds as String] as? [String: Any],
              let id = entry[kCGWindowNumber as String] as? Int else { return nil }
        let name = entry[kCGWindowOwnerName as String] as? String ?? ""
        if let owner, name != owner { return nil }
        let rect = CGRect(x: boundsDict["X"] as? Double ?? 0, y: boundsDict["Y"] as? Double ?? 0,
                          width: boundsDict["Width"] as? Double ?? 0, height: boundsDict["Height"] as? Double ?? 0)
        return WindowInfo(id: id, pid: pid, owner: name, layer: layer,
                          name: entry[kCGWindowName as String] as? String ?? "", bounds: rect)
    }
}

func onScreenWindows(_ pid: pid_t) -> [WindowInfo] {
    windowList().filter { $0.pid == pid && $0.bounds.width > 40 && $0.bounds.height > 40 }
}

// MARK: - Process helpers

func runningApplications(matching path: String) -> [NSRunningApplication] {
    let target = URL(fileURLWithPath: path).resolvingSymlinksInPath().standardizedFileURL.path
    return NSWorkspace.shared.runningApplications.filter { app in
        guard let exe = app.executableURL?.resolvingSymlinksInPath().standardizedFileURL.path else { return false }
        return exe == target
    }
}

/// Runs this binary as a child and returns its exit status and output. The parent
/// never queries AppKit or TIS itself: its view of both goes stale without a run
/// loop, and one of those calls trapped the probe outright.
func runSelf(_ arguments: [String], timeout: Double = 30) -> (status: Int32, out: String) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
    process.arguments = arguments
    process.standardInput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    let pipe = Pipe()
    process.standardOutput = pipe
    guard (try? process.run()) != nil else { return (-1, "") }
    let deadline = Date().addingTimeInterval(timeout)
    while process.isRunning && Date() < deadline {
        Thread.sleep(forTimeInterval: 0.05)
    }
    if process.isRunning {
        process.terminate()
        Thread.sleep(forTimeInterval: 0.5)
        if process.isRunning { kill(process.processIdentifier, SIGKILL) }
    }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    return (process.terminationStatus, String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines))
}

/// The frontmost application as a fresh process sees it. NSWorkspace caches the
/// value and updates it through notifications that need a run loop; this tool never
/// runs one, so its own read goes stale exactly like the input source did.
func frontmostProcess() -> (pid: pid_t, name: String) {
    let parts = runSelf(["front"], timeout: 10).out.split(separator: " ", maxSplits: 1)
    guard let first = parts.first, let pid = pid_t(first) else { return (0, "none") }
    return (pid, parts.count > 1 ? String(parts[1]) : "none")
}

func frontmostPid() -> pid_t { frontmostProcess().pid }

func frontmostName() -> String { frontmostProcess().name }

func activate(_ pid: pid_t) -> Bool {
    guard let app = NSRunningApplication(processIdentifier: pid) else { return false }
    let ok = app.activate(options: [.activateAllWindows])
    let element = axApp(pid)
    AXUIElementSetAttributeValue(element, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
    // The document window, not AXWindows.first: that can be an input-method
    // indicator bubble, and raising it leaves the real window behind.
    if let window = axDocumentWindow(pid) {
        AXUIElementPerformAction(window, kAXRaiseAction as CFString)
        AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString, kCFBooleanTrue)
    }
    return ok
}

func footprintBytes(_ pid: pid_t) -> UInt64? {
    var info = rusage_info_v4()
    let result = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V4, $0) }
    }
    return result == 0 ? info.ri_phys_footprint : nil
}

/// Velocity processes running `executable`; the GUI child is the one whose command
/// line carries the harness's --width flag (the CLI forwarder has none).
func topVelocityProcesses(executable: String) -> [(pid: pid_t, footprint: UInt64)] {
    let target = URL(fileURLWithPath: executable).resolvingSymlinksInPath().standardizedFileURL.path
    var pids: [pid_t] = []
    if let ps = output("ps -axo pid=,command=") {
        for line in ps.split(separator: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard let space = trimmed.firstIndex(of: " "), let pid = pid_t(trimmed[..<space]) else { continue }
            let command = String(trimmed[trimmed.index(after: space)...])
            let runsTarget = command == target || command.hasSuffix(" " + target) || command.hasPrefix(target + " ")
            if runsTarget && command.contains("--width") {
                pids.append(pid)
            }
        }
    }
    return pids.compactMap { pid in footprintBytes(pid).map { (pid, $0) } }
        .sorted { $0.1 > $1.1 }
}

// MARK: - Waiting

func sleep(seconds: Double) {
    Thread.sleep(forTimeInterval: seconds)
}

/// Polls `probe` until it returns true or the deadline passes. AX read errors from
/// polls that were retried are dropped (logged as a count); those of the final
/// poll, the one that decided the outcome, stay pending for the next check.
func waitUntil(_ timeout: Double, interval: Double = 0.1, _ probe: () -> Bool) -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    let errors = AXReadErrors.shared
    let mark = errors.mark
    var retried: [String] = []
    defer {
        if !retried.isEmpty { log("AX reads failed in earlier polls of a wait (retried): \(summarizeAXErrors(retried))") }
    }
    repeat {
        retried += errors.rollback(to: mark)
        if probe() { return true }
        Thread.sleep(forTimeInterval: interval)
    } while Date() < deadline
    return false
}

// MARK: - HID events

private let modifierFlags: [String: CGEventFlags] = [
    "cmd": .maskCommand, "command": .maskCommand, "shift": .maskShift,
    "opt": .maskAlternate, "alt": .maskAlternate, "ctrl": .maskControl, "control": .maskControl,
]

func modifierFlags(_ spec: String) -> CGEventFlags {
    var flags = CGEventFlags()
    for part in spec.split(separator: "+") where !part.isEmpty {
        if let flag = modifierFlags[part.lowercased()] { flags.insert(flag) }
    }
    return flags
}

/// US ANSI virtual key codes (HID usage is the layout's business; the IME and the
/// layout both see these, which is exactly what a physical key sends).
let keyCodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
    "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
    "3": 20, "4": 21, "6": 22, "5": 23, "9": 25, "7": 26, "8": 28, "0": 29, "o": 31,
    "u": 32, "i": 34, "p": 35, "return": 36, "enter": 36, "l": 37, "j": 38, "k": 40,
    "n": 45, "m": 46, "tab": 48, "space": 49, "`": 50, "delete": 51, "escape": 53,
    "esc": 53, "left": 123, "right": 124, "down": 125, "up": 126, "home": 115,
    "end": 119, "pageup": 116, "pagedown": 121, "forwarddelete": 117,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
    "-": 27, "=": 24, "[": 33, "]": 30, ";": 41, "'": 39, ",": 43, ".": 47, "/": 44,
    "\\": 42, "keypad_enter": 76,
]
private let shiftedCharacters: [Character: String] = [
    "A": "a", "S": "s", "D": "d", "F": "f", "H": "h", "G": "g", "Z": "z", "X": "x", "C": "c", "V": "v",
    "B": "b", "Q": "q", "W": "w", "E": "e", "R": "r", "Y": "y", "T": "t", "O": "o", "U": "u", "I": "i",
    "P": "p", "L": "l", "J": "j", "K": "k", "N": "n", "M": "m", "!": "1", "@": "2", "#": "3", "$": "4",
    "%": "5", "^": "6", "&": "7", "*": "8", "(": "9", ")": "0", "_": "-", "+": "=", "{": "[", "}": "]",
    ":": ";", "\"": "'", "<": ",", ">": ".", "?": "/", "|": "\\", "~": "`",
]

func postKey(_ code: CGKeyCode, flags: CGEventFlags = [], hold: UInt32 = 12_000) {
    let source = CGEventSource(stateID: .hidSystemState)
    guard let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false) else { return }
    down.flags = flags
    up.flags = flags
    down.post(tap: .cghidEventTap)
    usleep(hold)
    up.post(tap: .cghidEventTap)
    usleep(20_000)
}

func postKey(_ name: String, flags: CGEventFlags = []) {
    let lower = name.lowercased()
    guard let code = keyCodes[lower] else { fail("unknown key \(name)") }
    postKey(code, flags: flags)
}

/// HID key events for the US layout: letters go through the active input source,
/// which is what the IME needs to see.
func typeKeys(_ text: String, hold: UInt32 = 10_000, gap: UInt32 = 30_000) {
    for character in text {
        let key: String
        var flags = CGEventFlags()
        if let shifted = shiftedCharacters[character] {
            key = shifted
            flags.insert(.maskShift)
        } else if character == " " {
            key = "space"
        } else {
            key = String(character)
        }
        guard let code = keyCodes[key] else { fail("no keycode for \(character.debugDescription)") }
        let source = CGEventSource(stateID: .hidSystemState)
        guard let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false) else { continue }
        down.flags = flags
        up.flags = flags
        down.post(tap: .cghidEventTap)
        usleep(hold)
        up.post(tap: .cghidEventTap)
        usleep(gap)
    }
}

/// Unicode key events: bypass the layout and the input method (used for fixtures
/// such as setting a document name).
func typeUnicode(_ text: String, gap: UInt32 = 20_000) {
    let source = CGEventSource(stateID: .hidSystemState)
    for character in text.utf16 {
        var unit = character
        guard let down = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false) else { continue }
        down.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
        up.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
        // Explicitly no modifiers: a .hidSystemState event otherwise inherits the HID
        // modifier state, and after a Cmd+click that turned "XY" into Cmd+X, Cmd+Y.
        down.flags = []
        up.flags = []
        down.post(tap: .cghidEventTap)
        usleep(8_000)
        up.post(tap: .cghidEventTap)
        usleep(gap)
    }
}

/// HID pointer events, top-left screen coordinates (the same space as AXFrame and
/// CGWindowBounds). A move precedes every press so hover state and the window under
/// the pointer are live, as with a hand on the mouse.
func mouseMove(to point: CGPoint) {
    let source = CGEventSource(stateID: .hidSystemState)
    CGEvent(mouseEventSource: source, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left)?
        .post(tap: .cghidEventTap)
    usleep(15_000)
}

func mouseEvent(_ type: CGEventType, _ point: CGPoint, clicks: Int = 1, flags: CGEventFlags = []) {
    let source = CGEventSource(stateID: .hidSystemState)
    guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .left) else { return }
    event.setIntegerValueField(.mouseEventClickState, value: Int64(clicks))
    event.flags = flags
    event.post(tap: .cghidEventTap)
    usleep(25_000)
}

func click(at point: CGPoint, clicks: Int = 1, flags: CGEventFlags = []) {
    mouseMove(to: point)
    for clickState in 1...max(1, clicks) {
        mouseEvent(.leftMouseDown, point, clicks: clickState, flags: flags)
        mouseEvent(.leftMouseUp, point, clicks: clickState, flags: flags)
        usleep(clicks > 1 ? 40_000 : 80_000)
    }
    // A modifier carried on HID mouse events stays in the HID modifier state; release
    // it so later events built from .hidSystemState do not inherit it.
    if !flags.isEmpty {
        let source = CGEventSource(stateID: .hidSystemState)
        for (flag, key) in [(CGEventFlags.maskCommand, CGKeyCode(55)), (.maskShift, 56), (.maskAlternate, 58), (.maskControl, 59)]
        where flags.contains(flag) {
            guard let up = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: false) else { continue }
            up.type = .flagsChanged
            up.flags = []
            up.post(tap: .cghidEventTap)
            usleep(20_000)
        }
    }
}

func drag(from start: CGPoint, to end: CGPoint, steps: Int = 12, pause: Double = 0.05) {
    mouseMove(to: start)
    mouseEvent(.leftMouseDown, start)
    let dx = (end.x - start.x) / CGFloat(steps)
    let dy = (end.y - start.y) / CGFloat(steps)
    for step in 1...steps {
        let point = CGPoint(x: start.x + dx * CGFloat(step), y: start.y + dy * CGFloat(step))
        mouseEvent(.leftMouseDragged, point)
        Thread.sleep(forTimeInterval: pause)
    }
    // Hold still at the destination so a drop target can latch.
    Thread.sleep(forTimeInterval: 0.15)
    mouseEvent(.leftMouseUp, end)
    usleep(120_000)
}

func pointerLocation() -> CGPoint {
    CGEvent(source: nil)?.location ?? .zero
}

// MARK: - Input sources

func stringProperty(_ source: TISInputSource, _ key: String) -> String {
    guard let raw = TISGetInputSourceProperty(source, key as CFString) else { return "" }
    return String(describing: Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue())
}

func boolProperty(_ source: TISInputSource, _ key: String) -> Bool {
    (TISGetInputSourceProperty(source, key as CFString).map { Unmanaged<AnyObject>.fromOpaque($0).takeUnretainedValue() } as? NSNumber)?.boolValue ?? false
}

func inputSources(includeDisabled: Bool = true) -> [TISInputSource] {
    (TISCreateInputSourceList(nil, includeDisabled).takeRetainedValue() as? [TISInputSource]) ?? []
}

func localCurrentInputSourceId() -> String {
    stringProperty(TISCopyCurrentKeyboardInputSource().takeRetainedValue(), kTISPropertyInputSourceID as String)
}

/// TIS caches the current source per process and this tool never pumps a main loop,
/// so after another process switches the source its own view stays stale. Ask a
/// fresh process instead.
func currentInputSourceId() -> String {
    let text = runSelf(["current-input"], timeout: 10).out
    return text.isEmpty ? "unknown" : text
}

func findInputSource(_ id: String) -> TISInputSource? {
    let properties = [kTISPropertyInputSourceID as String: id] as CFDictionary
    let list = TISCreateInputSourceList(properties, true).takeRetainedValue() as? [TISInputSource]
    return list?.first
}

/// Only sources in the *enabled* list can be selected; a stale handle from the
/// "all installed" list is refused with paramErr (-50) even right after enabling.
func findEnabledInputSource(_ id: String) -> TISInputSource? {
    let properties = [kTISPropertyInputSourceID as String: id] as CFDictionary
    let list = TISCreateInputSourceList(properties, false).takeRetainedValue() as? [TISInputSource]
    return list?.first
}

func inputSourceInfo(_ id: String) -> String {
    guard let source = findInputSource(id) else { return "\(id): missing" }
    return "\(id) enabled=\(boolProperty(source, kTISPropertyInputSourceIsEnabled as String)) selectable=\(boolProperty(source, kTISPropertyInputSourceIsSelectCapable as String)) category=\(stringProperty(source, kTISPropertyInputSourceCategory as String)) type=\(stringProperty(source, kTISPropertyInputSourceType as String))"
}

/// The same fields one at a time, for breadcrumbs around a suspect property.
func inputSourceBasics(_ id: String) -> String {
    guard let source = findInputSource(id) else { return "\(id): missing" }
    return "\(id) enabled=\(boolProperty(source, kTISPropertyInputSourceIsEnabled as String)) selectable=\(boolProperty(source, kTISPropertyInputSourceIsSelectCapable as String))"
}

func enableInputSource(_ id: String) -> OSStatus {
    guard let source = findInputSource(id) else { return -1 }
    return TISEnableInputSource(source)
}

func selectInputSource(_ id: String) -> OSStatus {
    if let source = findEnabledInputSource(id) {
        return TISSelectInputSource(source)
    }
    guard let source = findInputSource(id) else { return -1 }
    _ = TISEnableInputSource(source)
    if let enabled = findEnabledInputSource(id) { return TISSelectInputSource(enabled) }
    return TISSelectInputSource(source)
}

let pinyinIds = [
    "com.apple.inputmethod.SCIM.ITABC",
    "com.apple.inputmethod.SCIM.SimplifiedPinyin",
    "com.apple.inputmethod.SCIM",
]
// US is enabled on the runner image; ABC is not, and enabling then selecting a
// disabled layout trapped the child once (exit 5 = SIGTRAP).
let abcIds = [
    "com.apple.keylayout.US",
    "com.apple.keylayout.ABC",
]

/// Switches the system input source through a child process and returns the id that
/// took effect (empty when none did).
@discardableResult
func useInputSource(_ ids: [String]) -> String {
    for id in ids {
        let result = runSelf(["switch-input", id], timeout: 20)
        log("switch-input \(id) exit=\(result.status) \(result.out)")
        if result.out.contains("current=\(id)") { return id }
    }
    return ""
}

let pinyinTarget = "com.apple.inputmethod.SCIM.ITABC"

/// Breadcrumb before a risky call: a trap leaves the last trace line in the log.
func trace(_ message: String) {
    print("LOG [ime] \(message)")
    fflush(stdout)
}

/// Enables and selects Pinyin. Runs in its own process (see `switch-ime`): the
/// input source APIs trapped once on a runner, and a child keeps the probe alive.
@discardableResult
func switchToPinyin(_ report: Report?) -> Bool {
    if currentInputSourceId() == pinyinTarget { return true }
    // standardOutput from the child lands in the same log stream.
    let process = Process()
    process.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
    process.arguments = ["switch-ime"]
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = FileHandle.standardOutput
    process.standardError = FileHandle.standardError
    do {
        try process.run()
    } catch {
        report?.note("IME child", detail: "spawn failed: \(error)")
        return false
    }
    _ = waitUntil(40, interval: 0.2) { !process.isRunning }
    if process.isRunning { process.terminate() }
    report?.note("IME child exit", detail: "\(process.terminationStatus)")
    return waitUntil(2) { currentInputSourceId() == pinyinTarget }
}

/// The switching itself: every step is traced before it runs, so a crash names
/// the call that caused it.
func switchInputMethodToPinyin() {
    trace("start target=\(pinyinTarget) current=\(localCurrentInputSourceId())")
    trace("info-basics \(inputSourceBasics(pinyinTarget))")
    trace("info-extra \(inputSourceInfo(pinyinTarget))")
    trace("enable method")
    let parent = enableInputSource("com.apple.inputmethod.SCIM")
    trace("enable method status=\(parent)")
    trace("enable mode")
    let mode = enableInputSource(pinyinTarget)
    trace("enable mode status=\(mode) now=\(inputSourceInfo(pinyinTarget))")
    for attempt in 1...3 {
        trace("select mode attempt \(attempt)")
        let status = selectInputSource(pinyinTarget)
        trace("select mode attempt \(attempt) status=\(status) current=\(localCurrentInputSourceId())")
        if waitUntil(1.5, interval: 0.1, { localCurrentInputSourceId() == pinyinTarget }) { return }
    }
    trace("select method")
    let parentStatus = selectInputSource("com.apple.inputmethod.SCIM")
    trace("select method status=\(parentStatus) current=\(localCurrentInputSourceId())")
    _ = waitUntil(1) { localCurrentInputSourceId().contains("SCIM") }
    trace("select mode after method")
    let again = selectInputSource(pinyinTarget)
    trace("select mode after method status=\(again) current=\(localCurrentInputSourceId())")
    if waitUntil(1.5, interval: 0.1, { localCurrentInputSourceId() == pinyinTarget }) { return }
    for step in 1...4 {
        trace("control+space \(step)")
        postKey("space", flags: .maskControl)
        sleep(seconds: 0.6)
        trace("control+space \(step) current=\(localCurrentInputSourceId())")
        if localCurrentInputSourceId() == pinyinTarget { return }
    }
    trace("defaults write")
    let list = "[{InputSourceKind = \"Input Mode\"; \"Bundle ID\" = \"com.apple.inputmethod.SCIM\"; \"Input Mode\" = \"ITABC\"; }]"
    let enabled = "[{InputSourceKind = \"Keyboard Layout\"; \"KeyboardLayout ID\" = 252; \"KeyboardLayout Name\" = ABC; }, {InputSourceKind = \"Input Method\"; \"Bundle ID\" = \"com.apple.inputmethod.SCIM\"; }, {InputSourceKind = \"Input Mode\"; \"Bundle ID\" = \"com.apple.inputmethod.SCIM\"; \"Input Mode\" = \"ITABC\"; }]"
    _ = shell("defaults write com.apple.HIToolbox AppleEnabledInputSources -array \(enabled) && defaults write com.apple.HIToolbox AppleSelectedInputSources -array \(list) && killall TextInputMenuAgent 2>/dev/null; true")
    _ = waitUntil(3) { localCurrentInputSourceId() == pinyinTarget }
    trace("end current=\(localCurrentInputSourceId())")
}
