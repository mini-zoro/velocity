// Black-box launch probe: time from process spawn to the first on-screen window of that pid,
// then (optionally) wait until the process is idle and report its physical footprint.
// Usage: probe <owner-name> <executable> <idle-wait-seconds> [args...]
// The window is matched by owner name among windows that did not exist before the launch,
// because an app may relaunch itself (LaunchServices) under a different pid.
import Foundation
import CoreGraphics

let argv = CommandLine.arguments
guard argv.count >= 4 else { print("usage: probe owner exe idleSeconds [args]"); exit(2) }
let owner = argv[1]
let exe = argv[2]
let idleWait = Double(argv[3]) ?? 0
func windows() -> [[String: Any]] {
    (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]]) ?? []
}
let before = Set(windows().compactMap { $0[kCGWindowNumber as String] as? Int })
let p = Process()
p.executableURL = URL(fileURLWithPath: exe)
p.arguments = Array(argv.dropFirst(4))
p.standardOutput = FileHandle.nullDevice
p.standardError = FileHandle.nullDevice
let t0 = DispatchTime.now().uptimeNanoseconds
try p.run()
var pid = p.processIdentifier
var windowMs: Double = -1
let deadline = t0 + 30_000_000_000
while DispatchTime.now().uptimeNanoseconds < deadline {
    if let w = windows().first(where: { ($0[kCGWindowOwnerName as String] as? String) == owner
            && !before.contains(($0[kCGWindowNumber as String] as? Int) ?? -1)
            && (($0[kCGWindowLayer as String] as? Int) ?? 1) == 0
            && ((($0[kCGWindowBounds as String] as? [String: Any])?["Width"] as? Double) ?? 0) > 200 }) {
        windowMs = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1e6
        pid = (w[kCGWindowOwnerPID as String] as? Int32) ?? pid
        break
    }
    usleep(1000)
}
func footprintMB(_ pid: Int32) -> Double {
    var info = rusage_info_v4()
    let r = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V4, $0) }
    }
    return r == 0 ? Double(info.ri_phys_footprint) / 1e6 : -1
}
func cpuNs(_ pid: Int32) -> UInt64 {
    var info = rusage_info_v4()
    let r = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V4, $0) }
    }
    return r == 0 ? info.ri_user_time + info.ri_system_time : 0
}
var footprint: Double = -1
var settledMs: Double = -1
if idleWait > 0 {
    // Idle = less than 2% of one core over a 1 s window, observed 3 times in a row.
    var quiet = 0
    var last = cpuNs(pid)
    let end = DispatchTime.now().uptimeNanoseconds + UInt64(idleWait * 1e9)
    while DispatchTime.now().uptimeNanoseconds < end && quiet < 3 {
        sleep(1)
        let now = cpuNs(pid)
        quiet = (now - last) < 20_000_000 ? quiet + 1 : 0
        last = now
    }
    settledMs = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1e6
    footprint = footprintMB(pid)
}
print(String(format: "window_ms=%.1f settled_ms=%.0f footprint_mb=%.1f", windowMs, settledMs, footprint))
print(String(format: "pid=%d", pid))
kill(pid, SIGTERM)
if p.isRunning { p.terminate() }
usleep(500_000)
kill(pid, SIGKILL)
