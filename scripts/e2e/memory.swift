// Memory gate: steady-state physical footprint of one, two and three editor windows
// after a fixed editing session in an isolated --data-dir.
//
// Every action goes through Velocity's own CLI (`--command`, file arguments, `-n`), so
// the scenario needs no HID access. Footprint is `ri_phys_footprint`, the number
// Activity Monitor shows as "Memory". The runner's display is 1024x768 at 1x, so its
// drawables are far smaller than on a Retina Mac; the report prints the IOSurface
// share separately so the result can be scaled to another display.
//
// Environment:
//   E2E_ROOT              repository checkout that supplies the project files
//   E2E_MEMORY_LAUNCHERS  "before=/a/velocity,after=/b/velocity": run each build, interleaved
//   E2E_MEMORY_ROUNDS     sessions per build (default 2)
//   E2E_MEMORY_LIMITS     "single=MiB,window=MiB" overrides the gate below
//   E2E_MEMORY_DIAG       1: drawable states, footprint/vmmap/heap dumps and a
//                         MallocStackLogging session for allocation attribution
import AppKit
import ApplicationServices
import Foundation

// One window on a 1000x680-point runner window. Set from the runner baseline plus a
// margin; see the memory gate notes in docs/performance.md.
let defaultSingleLimitMiB = 96.0
let defaultWindowLimitMiB = 16.0

struct MemoryBuild {
    let label: String
    let launcher: URL
}

struct MemorySample {
    var single: UInt64 = 0
    var two: UInt64 = 0
    var three: UInt64 = 0
    var categories: [String] = []
}

func mib(_ bytes: UInt64) -> String { String(format: "%.1f", Double(bytes) / 1_048_576) }
func mib(_ delta: Int64) -> String { String(format: "%+.1f", Double(delta) / 1_048_576) }

/// Category rows of `footprint` ("dirty clean reclaimable regions name"); sizes may be
/// bare byte counts or carry a unit.
func footprintRow(_ line: Substring) -> (name: String, dirty: UInt64, regions: Int)? {
    let units: [String: Double] = ["B": 1, "K": 1024, "KB": 1024, "M": 1_048_576, "MB": 1_048_576,
                                   "G": 1_073_741_824, "GB": 1_073_741_824]
    let tokens = line.split(separator: " ", omittingEmptySubsequences: true).map(String.init)
    var sizes: [UInt64] = []
    var i = 0
    while sizes.count < 3 && i < tokens.count {
        guard let value = Double(tokens[i]) else { return nil }
        if i + 1 < tokens.count, let unit = units[tokens[i + 1]] {
            sizes.append(UInt64(value * unit))
            i += 2
        } else {
            sizes.append(UInt64(value))
            i += 1
        }
    }
    guard sizes.count == 3, i + 1 < tokens.count, let regions = Int(tokens[i]) else { return nil }
    return (tokens[(i + 1)...].joined(separator: " "), sizes[0], regions)
}

/// `footprint` needs the target's task port; GitHub runners have passwordless sudo.
func inspector(_ tool: String) -> String {
    shell("sudo -n true").status == 0 ? "sudo -n \(tool)" : tool
}

func footprintCategories(_ pid: pid_t, save path: String? = nil) -> [String: (dirty: UInt64, regions: Int)] {
    let text = shell("\(inspector("/usr/bin/footprint")) -f bytes \(pid) 2>&1").out
    if let path { try? text.write(toFile: path, atomically: true, encoding: .utf8) }
    var result: [String: (dirty: UInt64, regions: Int)] = [:]
    for line in text.split(separator: "\n") {
        if let row = footprintRow(line) { result[row.name] = (dirty: row.dirty, regions: row.regions) }
    }
    return result
}

/// Every category of at least 0.5 MiB, largest first, then the rest as one number.
func describeCategories(_ categories: [String: (dirty: UInt64, regions: Int)]) -> String {
    let sorted = categories.sorted { $0.value.dirty > $1.value.dirty }
    var parts = sorted.filter { $0.value.dirty >= 512 * 1024 }
        .map { "\($0.key)=\(mib($0.value.dirty))(\($0.value.regions))" }
    let rest = sorted.filter { $0.value.dirty < 512 * 1024 }.map(\.value.dirty).reduce(0, +)
    parts.append("rest=\(mib(rest))")
    return parts.isEmpty ? "unavailable" : parts.joined(separator: " ")
}

final class MemorySession {
    let build: MemoryBuild
    let work: URL
    let data: URL
    let logs: URL
    let project: URL
    let files: [String]
    let report: Report
    let extraEnvironment: [String: String]
    var pid: pid_t = 0

    /// Every session edits and saves a fresh copy of the project, so all sessions
    /// (and both builds of an A/B run) start from the same bytes.
    init(build: MemoryBuild, name: String, root: URL, repository: URL, report: Report,
         environment: [String: String] = [:], settings: String? = nil) {
        self.build = build
        work = root.appendingPathComponent(name)
        data = work.appendingPathComponent("data")
        logs = work.appendingPathComponent("logs")
        project = work.appendingPathComponent("project")
        self.report = report
        extraEnvironment = environment
        try? FileManager.default.removeItem(at: work)
        try? FileManager.default.createDirectory(at: data, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true)
        files = memoryProject(root: repository, into: project)
        if let settings {
            let user = data.appendingPathComponent("Packages/User")
            try? FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
            try? settings.write(to: user.appendingPathComponent("Preferences.sublime-settings"),
                                atomically: true, encoding: .utf8)
        }
    }

    var executable: String {
        // A bare developer binary; the packaged wrapper is not used here.
        build.launcher.resolvingSymlinksInPath().path
    }

    func environment() -> [String: String] {
        var env = ProcessInfo.processInfo.environment
        env["VELOCITY_LOG_DIR"] = logs.path
        for (key, value) in extraEnvironment { env[key] = value }
        return env
    }

    /// Runs the CLI against this session's instance and waits for it to finish.
    @discardableResult
    func cli(_ arguments: [String], timeout: Double = 30) -> Int32 {
        let process = Process()
        process.executableURL = build.launcher
        process.arguments = ["--data-dir", data.path] + arguments
        process.environment = environment()
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        let errors = logs.appendingPathComponent("cli.log")
        if !FileManager.default.fileExists(atPath: errors.path) {
            FileManager.default.createFile(atPath: errors.path, contents: nil)
        }
        let handle = try? FileHandle(forWritingTo: errors)
        handle?.seekToEndOfFile()
        process.standardError = handle ?? FileHandle.nullDevice
        guard (try? process.run()) != nil else { return -1 }
        let deadline = Date().addingTimeInterval(timeout)
        while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.01) }
        if process.isRunning {
            process.terminate()
            report.note("cli timeout", detail: arguments.joined(separator: " "))
            return -1
        }
        return process.terminationStatus
    }

    func command(_ name: String, _ args: String = "") {
        cli(["--command", args.isEmpty ? name : "\(name) \(args)"])
    }

    func launch(_ arguments: [String]) -> Bool {
        let process = Process()
        process.executableURL = build.launcher
        process.arguments = ["--data-dir", data.path, "--width", "1000", "--height", "680"] + arguments
        process.environment = environment()
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return false }
        let found = waitUntil(45, interval: 0.2) {
            guard let gui = topVelocityProcesses(executable: executable).first.map(\.pid),
                  !onScreenWindows(gui).isEmpty else { return false }
            pid = gui
            return true
        }
        return found
    }

    func windows() -> Int { onScreenWindows(pid).count }

    func footprint() -> UInt64 { footprintBytes(pid) ?? 0 }

    /// Samples until six consecutive readings 0.5 s apart stay within 512 KiB, after at
    /// least `minimum` seconds: the drawable pool trims 350 ms after the last frame, and
    /// index, highlight and allocator work settle within a few seconds.
    func settle(minimum: Double = 4, timeout: Double = 45) -> UInt64 {
        sleep(seconds: minimum)
        var readings: [UInt64] = [footprint()]
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            sleep(seconds: 0.5)
            readings.append(footprint())
            let last = readings.suffix(6)
            if last.count == 6, let low = last.min(), let high = last.max(), high - low <= 512 * 1024 {
                return last.last!
            }
        }
        report.note("footprint did not settle", detail: readings.suffix(6).map { mib($0) }.joined(separator: ", "))
        return readings.last ?? 0
    }

    func path(_ index: Int) -> String { files[index % files.count] }

    /// Scroll, type, find and switch tabs in the current window's active view.
    func edit(intensity: Int) {
        command("move_to", #"{"to":"bof"}"#)
        for _ in 0..<(4 * intensity) { command("move", #"{"by":"pages","forward":true}"#) }
        command("move_to", #"{"to":"eof"}"#)
        command("move_to", #"{"to":"bof"}"#)
        for i in 0..<(3 * intensity) {
            command("insert", #"{"characters":"// memory gate line \#(i): the quick brown fox\n"}"#)
        }
        command("move", #"{"by":"lines","forward":true}"#)
        command("find_under_expand")
        command("find_under_expand")
        command("single_selection")
        command("show_panel", #"{"panel":"find"}"#)
        command("find_next")
        command("find_next")
        command("hide_panel")
        command("show_overlay", #"{"overlay":"goto","show_files":true}"#)
        command("hide_overlay")
        command("save")
    }

    /// The fixed session: a project with ten tabs, then two more windows.
    func run(diagnostics: Bool) -> MemorySample? {
        var sample = MemorySample()
        guard launch([project.path, path(0)]) else {
            report.check(false, "\(build.label): window appeared")
            return nil
        }
        sleep(seconds: 1.5)
        cli((1..<10).map { path($0) })
        sleep(seconds: 1.5)
        edit(intensity: 3)
        for _ in 0..<12 { command("next_view") }
        for _ in 0..<6 { command("prev_view") }
        edit(intensity: 1)
        // Close five clean tabs (the edited one was saved).
        command("select_by_index", #"{"index":0}"#)
        for _ in 0..<5 { command("close") }
        sample.single = settle()
        let one = footprintCategories(pid, save: diagnostics ? logs.appendingPathComponent("footprint-1w.txt").path : nil)
        sample.categories.append("1w \(describeCategories(one))")

        cli(["-n", path(10)])
        _ = waitUntil(15) { windows() >= 2 }
        edit(intensity: 1)
        sample.two = settle()
        let two = footprintCategories(pid)
        sample.categories.append("2w \(describeCategories(two))")

        cli(["-n", path(11)])
        _ = waitUntil(15) { windows() >= 3 }
        edit(intensity: 1)
        sample.three = settle()
        let three = footprintCategories(pid, save: diagnostics ? logs.appendingPathComponent("footprint-3w.txt").path : nil)
        sample.categories.append("3w \(describeCategories(three))")
        if windows() < 3 { report.note("\(build.label): window count", detail: "\(windows()) on screen") }
        if diagnostics { drawableStates() }
        return sample
    }

    func surfaces() -> String {
        let categories = footprintCategories(pid)
        let surface = categories["IOSurface"].map { "\(mib($0.dirty)) MiB in \($0.regions)" } ?? "none"
        let graphics = categories["Owned physical footprint (graphics)"].map { mib($0.dirty) } ?? "-"
        return "footprint \(mib(footprint())) IOSurface \(surface) graphics \(graphics)"
    }

    /// Drawables and window-sized textures in each visibility state (diagnostic only).
    func drawableStates() {
        report.note("\(build.label) drawables idle", detail: surfaces())
        // Opening and closing a tab animates the tab strip: a burst of display-link
        // frames, which raises the pool to three drawables.
        for _ in 0..<2 {
            command("new_file")
            command("close")
        }
        report.note("\(build.label) drawables right after a tab-strip burst", detail: surfaces())
        sleep(seconds: 1)
        report.note("\(build.label) drawables 1 s after the burst", detail: surfaces())
        sleep(seconds: 3)
        report.note("\(build.label) drawables 4 s after the burst", detail: surfaces())
        command("move", #"{"by":"lines","forward":true}"#)
        sleep(seconds: 2)
        report.note("\(build.label) drawables after one isolated frame", detail: surfaces())
        let app = NSRunningApplication(processIdentifier: pid)
        // A fresh helper process reads the AX tree (this one never runs a loop).
        let minimized = runSelf(["minimize", "\(pid)", "1"]).out
        sleep(seconds: 4)
        report.note("\(build.label) drawables one window minimized 4 s", detail: "\(minimized) \(surfaces())")
        let restored = runSelf(["minimize", "\(pid)", "0"]).out
        sleep(seconds: 3)
        report.note("\(build.label) drawables after restoring it", detail: "\(restored) \(surfaces())")
        app?.hide()
        sleep(seconds: 4)
        report.note("\(build.label) drawables app hidden 4 s", detail: surfaces())
        app?.unhide()
        sleep(seconds: 3)
        report.note("\(build.label) drawables after unhide", detail: surfaces())
        command("close_window")
        command("close_window")
        let closed = settle()
        report.note("\(build.label) after closing two windows", detail: "\(mib(closed)) MiB, \(surfaces())")
    }

    func dump(_ name: String, extra: Bool) {
        let prefix = logs.appendingPathComponent(name).path
        _ = shell("\(inspector("/usr/bin/vmmap")) -summary \(pid) > '\(prefix)-vmmap.txt' 2>&1")
        _ = shell("\(inspector("/usr/bin/heap")) -s \(pid) > '\(prefix)-heap.txt' 2>&1")
        if extra {
            _ = shell("\(inspector("/usr/bin/malloc_history")) \(pid) -callTree -ignoreThreads -collapseRecursion > '\(prefix)-calltree.txt' 2>&1")
            _ = shell("\(inspector("/usr/bin/malloc_history")) \(pid) -callTree -invert -ignoreThreads -collapseRecursion > '\(prefix)-calltree-inverted.txt' 2>&1")
        }
    }

    func quit() {
        if pid != 0 { kill(pid, SIGTERM) }
        _ = waitUntil(10) { pid == 0 || kill(pid, 0) != 0 }
        if pid != 0 && kill(pid, 0) == 0 { kill(pid, SIGKILL) }
        sleep(seconds: 0.5)
    }

    /// Keeps the session's logs and dumps for the workflow artifact.
    func archive(_ artifacts: String, as name: String) {
        let target = "\(artifacts)/memory/\(name)"
        _ = shell("mkdir -p '\(target)' && cp -R '\(logs.path)/.' '\(target)/' 2>/dev/null; true")
    }
}

/// Copies twelve of the repository's own sources into a small Git project.
func memoryProject(root: URL, into dir: URL) -> [String] {
    let names = [
        "crates/vel-render/src/gpu.rs", "crates/vel-render/src/font.rs", "crates/velocity/src/cli.rs",
        "crates/vel-render/src/atlas.rs", "crates/vel-platform/src/memory_guard.rs", "docs/behavior.md",
        "Cargo.toml", ".github/workflows/ci.yml", "scripts/e2e/run.sh", "scripts/bench-headless.py",
        "scripts/e2e/ax-support.swift", "crates/vel-render/src/quads.wgsl",
    ]
    try? FileManager.default.removeItem(at: dir)
    var paths: [String] = []
    for name in names {
        let source = root.appendingPathComponent(name)
        let target = dir.appendingPathComponent(name)
        try? FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
        if (try? FileManager.default.copyItem(at: source, to: target)) != nil { paths.append(target.path) }
    }
    _ = shell("cd '\(dir.path)' && git init -q && git add -A && git -c user.name=e2e -c user.email=e2e@example.invalid commit -qm fixture")
    return paths
}

/// E2E_MEMORY_LAUNCHERS pairs, else the build under test (E2E_VELOCITY).
func memoryBuilds(_ env: [String: String], root: URL) -> [MemoryBuild] {
    let pairs = (env["E2E_MEMORY_LAUNCHERS"] ?? "").split(separator: ",").compactMap { pair -> MemoryBuild? in
        let parts = pair.split(separator: "=", maxSplits: 1).map(String.init)
        return parts.count == 2 ? MemoryBuild(label: parts[0], launcher: URL(fileURLWithPath: parts[1])) : nil
    }
    if !pairs.isEmpty { return pairs }
    let current = env["E2E_VELOCITY"] ?? root.appendingPathComponent("target/release/velocity").path
    return [MemoryBuild(label: "current", launcher: URL(fileURLWithPath: current))]
}

func scenarioMemory(_ options: ArraySlice<String>) -> Never {
    let report = Report("memory")
    guard let workPath = options.first else { fail("scenario needs a work directory") }
    let env = ProcessInfo.processInfo.environment
    let work = URL(fileURLWithPath: workPath).deletingLastPathComponent()
    let artifacts = env["E2E_ARTIFACTS"] ?? "/tmp"
    let root = URL(fileURLWithPath: env["E2E_ROOT"] ?? FileManager.default.currentDirectoryPath)
    let diagnostics = env["E2E_MEMORY_DIAG"] == "1"
    if diagnostics && !AXIsProcessTrusted() {
        // Minimizing a window goes through AX; the probe normally grants this.
        report.info("AX grant", detail: grantAccessibility(to: CommandLine.arguments[0]))
    }
    let explicitRounds = Int(env["E2E_MEMORY_ROUNDS"] ?? "")
    var singleLimit = defaultSingleLimitMiB
    var windowLimit = defaultWindowLimitMiB
    for pair in (env["E2E_MEMORY_LIMITS"] ?? "").split(separator: ",") {
        let parts = pair.split(separator: "=", maxSplits: 1).map(String.init)
        guard parts.count == 2, let value = Double(parts[1]) else { continue }
        if parts[0] == "single" { singleLimit = value }
        if parts[0] == "window" { windowLimit = value }
    }
    let builds = memoryBuilds(env, root: root)
    // An A/B comparison takes the median of three sessions per build.
    let rounds = max(1, explicitRounds ?? (builds.count > 1 ? 3 : 2))
    report.info("screens", detail: NSScreen.screens.map { "\($0.frame.width)x\($0.frame.height)@\($0.backingScaleFactor)x" }.joined(separator: ", "))
    report.info("builds", detail: "\(builds.count): " + builds.map { "\($0.label)=\($0.launcher.path)" }.joined(separator: " ")
        + " (E2E_MEMORY_LAUNCHERS=\(env["E2E_MEMORY_LAUNCHERS"] ?? "unset") E2E_VELOCITY=\(env["E2E_VELOCITY"] ?? "unset") rounds=\(rounds))")

    var results: [String: [MemorySample]] = [:]
    for round in 1...rounds {
        for build in builds {
            _ = shell("pkill -x velocity 2>/dev/null; true")
            sleep(seconds: 0.5)
            let name = "\(build.label)-\(round)"
            // The diagnostic round also traces the drawable pool to the GUI log.
            let session = MemorySession(build: build, name: "memory-\(name)", root: work, repository: root,
                                        report: report,
                                        environment: diagnostics && round == 1 ? ["VELOCITY_LOG_DRAWABLES": "1"] : [:])
            if session.files.count < 12 {
                report.check(false, "the fixture project has twelve files", detail: "\(session.files.count)")
            }
            if let sample = session.run(diagnostics: diagnostics && round == 1) {
                results[build.label, default: []].append(sample)
                report.note("\(name)", detail: "1w \(mib(sample.single)) MiB, 2w \(mib(sample.two)) (\(mib(Int64(sample.two) - Int64(sample.single)))), 3w \(mib(sample.three)) (\(mib(Int64(sample.three) - Int64(sample.two))))")
                for line in sample.categories { report.note("\(name) categories", detail: line) }
                if diagnostics && round == 1 { session.dump("heap", extra: false) }
            }
            session.quit()
            session.archive(artifacts, as: name)
        }
    }
    if diagnostics {
        // The Simplified Chinese UI needs a CJK fallback face for every label.
        report.info("CJK fonts", detail: shell("ls /System/Library/Fonts | grep -i -E 'ping|hei'; ls -d /System/Library/AssetsV2/com_apple_MobileAsset_Font*/*/AssetData/PingFang* 2>/dev/null; true").out.replacingOccurrences(of: "\n", with: " | "))
        for build in builds {
            _ = shell("pkill -x velocity 2>/dev/null; true")
            let chinese = MemorySession(build: build, name: "memory-\(build.label)-zh", root: work, repository: root,
                                        report: report, settings: #"{"ui_language": "zh-Hans"}"#)
            if let sample = chinese.run(diagnostics: false) {
                report.note("\(build.label) zh-Hans UI", detail: "1w \(mib(sample.single)) MiB, 2w \(mib(sample.two)), 3w \(mib(sample.three))")
                for line in sample.categories { report.note("\(build.label) zh-Hans categories", detail: line) }
                chinese.dump("zh-heap", extra: false)
            }
            chinese.quit()
            chinese.archive(artifacts, as: "\(build.label)-zh")
        }
    }
    if diagnostics, let build = builds.last {
        // Allocation call trees; MallocStackLogging itself costs memory, so this
        // session is never part of the gate.
        _ = shell("pkill -x velocity 2>/dev/null; true")
        let session = MemorySession(build: build, name: "memory-\(build.label)-stacks", root: work, repository: root,
                                    report: report, environment: ["MallocStackLogging": "lite"])
        if let sample = session.run(diagnostics: false) {
            report.note("\(build.label) with MallocStackLogging", detail: "1w \(mib(sample.single)) 3w \(mib(sample.three))")
            session.dump("stacks", extra: true)
        }
        session.quit()
        session.archive(artifacts, as: "\(build.label)-stacks")
    }

    func median(_ values: [UInt64]) -> UInt64 {
        let sorted = values.sorted()
        guard !sorted.isEmpty else { return 0 }
        return sorted.count % 2 == 1 ? sorted[sorted.count / 2]
            : (sorted[sorted.count / 2 - 1] + sorted[sorted.count / 2]) / 2
    }
    var medians: [String: (UInt64, Int64, Int64)] = [:]
    for build in builds {
        let samples = results[build.label] ?? []
        guard !samples.isEmpty else { continue }
        let single = median(samples.map(\.single))
        let second = median(samples.map { UInt64(max(0, Int64($0.two) - Int64($0.single))) })
        let third = median(samples.map { UInt64(max(0, Int64($0.three) - Int64($0.two))) })
        medians[build.label] = (single, Int64(second), Int64(third))
        report.note("\(build.label) median of \(samples.count)", detail: "1 window \(mib(single)) MiB, 2nd window +\(mib(second)), 3rd window +\(mib(third))")
    }
    if let before = medians["before"], let after = medians["after"] {
        report.note("after vs before", detail: "1 window \(mib(Int64(after.0) - Int64(before.0))) MiB, 2nd window \(mib(after.1 - before.1)), 3rd window \(mib(after.2 - before.2))")
    }
    let gated = builds.last.map(\.label) ?? "current"
    if let gate = medians[gated] {
        let (single, second, third) = gate
        report.check(Double(single) / 1_048_576 <= singleLimit, "\(gated): one window ≤ \(singleLimit) MiB", detail: "\(mib(single)) MiB")
        report.check(Double(second) / 1_048_576 <= windowLimit, "\(gated): second window adds ≤ \(windowLimit) MiB", detail: "+\(mib(UInt64(second))) MiB")
        report.check(Double(third) / 1_048_576 <= windowLimit, "\(gated): third window adds ≤ \(windowLimit) MiB", detail: "+\(mib(UInt64(third))) MiB")
    } else {
        report.check(false, "\(gated): a complete memory session")
    }
    _ = shell("pkill -x velocity 2>/dev/null; true")
    report.finish()
}
