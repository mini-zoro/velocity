// Startup scenario: what the screen shows between a cold launch and the first
// stable frame.
//
// The display is recorded with ScreenCaptureKit (every composited frame, with its
// display time) from before the launch until the window has been stable for a
// while. Inside the final window rectangles every frame is then judged against two
// references: the screen before the launch (B) and the final frame (S). A pixel
// whose colour is neither B, S nor anything between them, per channel and with a
// small tolerance, is "foreign". A fade or a scale animation from the desktop to
// the content stays inside that envelope; an empty window painted in a system
// colour, or a frame painted in another theme, does not.
import AppKit
import CoreGraphics
import CoreMedia
import CoreVideo
import Foundation
import ImageIO
import ScreenCaptureKit

// MARK: - Clock

private let machTimebase: mach_timebase_info_data_t = {
    var info = mach_timebase_info_data_t()
    mach_timebase_info(&info)
    return info
}()

func machSeconds(_ ticks: UInt64) -> Double {
    Double(ticks) * Double(machTimebase.numer) / Double(machTimebase.denom) / 1e9
}

/// Seconds on the host clock: mach_absolute_time, the clock of display times.
func hostNow() -> Double { machSeconds(mach_absolute_time()) }

/// Wall-clock seconds to host seconds, for process start times.
func hostTime(wall: Double) -> Double {
    wall - (Date().timeIntervalSince1970 - hostNow())
}

final class Box<T>: @unchecked Sendable {
    var value: T
    init(_ value: T) { self.value = value }
}

// MARK: - Frames

final class ScreenFrame {
    let time: Double
    let width: Int
    let height: Int
    let stride: Int
    let pixels: [UInt8] // BGRA rows, top first

    init(time: Double, width: Int, height: Int, stride: Int, pixels: [UInt8]) {
        self.time = time
        self.width = width
        self.height = height
        self.stride = stride
        self.pixels = pixels
    }

    func sameImage(as other: ScreenFrame) -> Bool {
        width == other.width && height == other.height && stride == other.stride && pixels == other.pixels
    }
}

private let sRGB = CGColorSpace(name: CGColorSpace.sRGB)!

func bgraPixels(_ image: CGImage) -> (width: Int, height: Int, pixels: [UInt8])? {
    let width = image.width, height = image.height
    guard width > 0, height > 0 else { return nil }
    var pixels = [UInt8](repeating: 0, count: width * height * 4)
    let drawn = pixels.withUnsafeMutableBytes { raw -> Bool in
        guard let context = CGContext(
            data: raw.baseAddress, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: sRGB,
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        else { return false }
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return true
    }
    return drawn ? (width, height, pixels) : nil
}

/// Writes the frame (optionally cropped, in pixels) as a PNG.
@discardableResult
func writePNG(_ frame: ScreenFrame, crop: CGRect?, to path: String) -> Bool {
    guard let provider = CGDataProvider(data: Data(frame.pixels) as CFData),
          let full = CGImage(
              width: frame.width, height: frame.height, bitsPerComponent: 8, bitsPerPixel: 32,
              bytesPerRow: frame.stride, space: sRGB,
              bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue
                  | CGBitmapInfo.byteOrder32Little.rawValue),
              provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    else { return false }
    let image = crop.flatMap { full.cropping(to: $0) } ?? full
    let url = URL(fileURLWithPath: path)
    try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil)
    else { return false }
    CGImageDestinationAddImage(destination, image, nil)
    return CGImageDestinationFinalize(destination)
}

// CGWindowListCreateImage is obsoleted for Swift in the macOS 15 SDK but still
// served by the window server; it is the fallback screen source and the only
// source of a single window's own backing image.
private typealias WindowListCreateImage = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
private let legacyCreateImage: WindowListCreateImage? = {
    guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else { return nil }
    return unsafeBitCast(symbol, to: WindowListCreateImage.self)
}()

/// The window's own backing image (what the window server holds for it), nominal
/// resolution, frame excluded.
func windowOwnImage(_ id: Int) -> ScreenFrame? {
    // kCGWindowListOptionIncludingWindow = 8; BoundsIgnoreFraming = 1, NominalResolution = 16.
    let time = hostNow()
    guard let create = legacyCreateImage, let image = create(.null, 8, UInt32(id), 1 | 16)?.takeRetainedValue(),
          let pixels = bgraPixels(image) else { return nil }
    return ScreenFrame(time: time, width: pixels.width, height: pixels.height, stride: pixels.width * 4,
                       pixels: pixels.pixels)
}

// MARK: - Screen recorder

final class ScreenRecorder: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var frames: [ScreenFrame] = []
    private var stream: SCStream?
    private let queue = DispatchQueue(label: "velocity.e2e.startup.capture", qos: .userInteractive)
    private var polling = false
    private var poller: Thread?
    private(set) var method = "none"
    private(set) var failure = ""
    private var stopped: String?
    private var droppedFrames = 0
    let display = CGMainDisplayID()
    /// Frames kept per recording; 3 MB each on the 1024x768 runner.
    let limit = 260

    var scale: Double {
        let bounds = CGDisplayBounds(display)
        guard let mode = CGDisplayCopyDisplayMode(display), bounds.width > 0 else { return 1 }
        return Double(mode.pixelWidth) / Double(bounds.width)
    }

    var refreshRate: Double { CGDisplayCopyDisplayMode(display)?.refreshRate ?? 0 }

    func start() -> Bool {
        if startStream() {
            method = "ScreenCaptureKit stream"
            return true
        }
        let streamFailure = failure
        if startPolling() {
            method = "CGWindowListCreateImage poll (stream failed: \(streamFailure))"
            return true
        }
        failure = "stream: \(streamFailure); poll: \(failure)"
        return false
    }

    func stop() {
        polling = false
        if let stream {
            let done = DispatchSemaphore(value: 0)
            stream.stopCapture { _ in done.signal() }
            _ = done.wait(timeout: .now() + 5)
        }
        stream = nil
    }

    func clear() {
        lock.lock()
        frames.removeAll(keepingCapacity: true)
        droppedFrames = 0
        lock.unlock()
    }

    func take() -> (frames: [ScreenFrame], dropped: Int, stopped: String?) {
        lock.lock()
        defer { lock.unlock() }
        return (frames, droppedFrames, stopped)
    }

    var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return frames.count
    }

    private func append(_ frame: ScreenFrame) {
        lock.lock()
        defer { lock.unlock() }
        if let last = frames.last, last.sameImage(as: frame) { return }
        if frames.count < limit {
            frames.append(frame)
        } else {
            droppedFrames += 1
        }
    }

    /// A still screen produces no new frame; the last one is re-sent on request so
    /// every recording starts with the screen before the launch.
    private var latest: ScreenFrame?
    func seedWithLatest() {
        lock.lock()
        let frame = latest
        lock.unlock()
        if let frame { append(frame) }
    }

    private func startStream() -> Bool {
        let done = DispatchSemaphore(value: 0)
        let found = Box<(SCShareableContent?, Error?)>((nil, nil))
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
            found.value = (content, error)
            done.signal()
        }
        guard done.wait(timeout: .now() + 15) == .success else {
            failure = "SCShareableContent timed out"
            return false
        }
        guard let content = found.value.0,
              let target = content.displays.first(where: { $0.displayID == display }) ?? content.displays.first
        else {
            failure = "SCShareableContent: \(found.value.1.map { "\($0)" } ?? "no display")"
            return false
        }
        let filter = SCContentFilter(display: target, excludingWindows: [])
        let configuration = SCStreamConfiguration()
        configuration.width = Int((Double(target.width) * scale).rounded())
        configuration.height = Int((Double(target.height) * scale).rounded())
        configuration.minimumFrameInterval = CMTime(value: 1, timescale: 120)
        configuration.pixelFormat = kCVPixelFormatType_32BGRA
        configuration.colorSpaceName = CGColorSpace.sRGB
        configuration.showsCursor = false
        configuration.queueDepth = 8
        let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
        do {
            try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        } catch {
            failure = "addStreamOutput: \(error)"
            return false
        }
        let started = DispatchSemaphore(value: 0)
        let startError = Box<Error?>(nil)
        stream.startCapture { error in
            startError.value = error
            started.signal()
        }
        guard started.wait(timeout: .now() + 15) == .success else {
            failure = "startCapture timed out"
            return false
        }
        if let error = startError.value {
            failure = "startCapture: \(error)"
            return false
        }
        self.stream = stream
        guard waitUntil(5, interval: 0.05, { self.count > 0 }) else {
            failure = "no frame within 5 s"
            stop()
            return false
        }
        return true
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sampleBuffer.isValid,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
                as? [[SCStreamFrameInfo: Any]],
              let info = attachments.first,
              let raw = info[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete,
              let buffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        let time: Double
        if let ticks = info[.displayTime] as? UInt64 {
            time = machSeconds(ticks)
        } else {
            time = CMSampleBufferGetPresentationTimeStamp(sampleBuffer).seconds
        }
        CVPixelBufferLockBaseAddress(buffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return }
        let width = CVPixelBufferGetWidth(buffer), height = CVPixelBufferGetHeight(buffer)
        let stride = CVPixelBufferGetBytesPerRow(buffer)
        let pixels = [UInt8](UnsafeRawBufferPointer(start: base, count: stride * height))
        let frame = ScreenFrame(time: time, width: width, height: height, stride: stride, pixels: pixels)
        lock.lock()
        latest = frame
        lock.unlock()
        append(frame)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        lock.lock()
        stopped = "\(error)"
        lock.unlock()
    }

    private func startPolling() -> Bool {
        guard let create = legacyCreateImage else {
            failure = "CGWindowListCreateImage is not available"
            return false
        }
        let bounds = CGDisplayBounds(display)
        func grab() -> ScreenFrame? {
            let time = hostNow()
            // kCGWindowListOptionOnScreenOnly = 1; kCGWindowImageNominalResolution = 16.
            guard let image = create(bounds, 1, 0, 16)?.takeRetainedValue(), let pixels = bgraPixels(image)
            else { return nil }
            return ScreenFrame(time: time, width: pixels.width, height: pixels.height, stride: pixels.width * 4,
                               pixels: pixels.pixels)
        }
        guard let first = grab() else {
            failure = "CGWindowListCreateImage returned no image"
            return false
        }
        latest = first
        append(first)
        polling = true
        let thread = Thread { [self] in
            while polling {
                if let frame = grab() {
                    lock.lock()
                    latest = frame
                    lock.unlock()
                    append(frame)
                }
            }
        }
        thread.qualityOfService = .userInteractive
        thread.start()
        poller = thread
        return true
    }
}

// MARK: - Windows and processes

struct SeenWindow {
    let id: Int
    let pid: pid_t
    var bounds: CGRect
    let created: Double
    var onscreen: Double?
    var alpha: Double
    var ownImages: [ScreenFrame] = []
}

/// Velocity's layer-0 windows, on screen or not, from one window-list read.
func velocityWindowRows(owner: String) -> [(id: Int, pid: pid_t, bounds: CGRect, onscreen: Bool, alpha: Double)] {
    let raw = (CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]]) ?? []
    return raw.compactMap { entry in
        guard (entry[kCGWindowOwnerName as String] as? String) == owner,
              (entry[kCGWindowLayer as String] as? Int) == 0,
              let pid = entry[kCGWindowOwnerPID as String] as? pid_t,
              let id = entry[kCGWindowNumber as String] as? Int,
              let box = entry[kCGWindowBounds as String] as? [String: Any] else { return nil }
        let bounds = CGRect(x: box["X"] as? Double ?? 0, y: box["Y"] as? Double ?? 0,
                            width: box["Width"] as? Double ?? 0, height: box["Height"] as? Double ?? 0)
        guard bounds.width > 100, bounds.height > 100 else { return nil }
        return (id, pid, bounds, (entry[kCGWindowIsOnscreen as String] as? Bool) ?? false,
                entry[kCGWindowAlpha as String] as? Double ?? 1)
    }
}

func processStartHost(_ pid: pid_t) -> Double? {
    var info = proc_bsdinfo()
    let size = Int32(MemoryLayout<proc_bsdinfo>.size)
    guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
    return hostTime(wall: Double(info.pbi_start_tvsec) + Double(info.pbi_start_tvusec) / 1e6)
}

func processAlive(_ pid: pid_t) -> Bool { kill(pid, 0) == 0 }

// MARK: - Judging frames

struct PixelBox {
    let x0: Int, y0: Int, x1: Int, y1: Int
    var cgRect: CGRect { CGRect(x: x0, y: y0, width: x1 - x0, height: y1 - y0) }
}

struct Judgement {
    let time: Double
    let changed: Double   // differs from the screen before the launch
    let matching: Double  // equals the final frame
    let foreign: Double   // outside the before/final envelope
    let white: Double     // near-pure white (every channel >= 250)
    let foreignColor: String
    let top: String
}

func hex(_ r: Int, _ g: Int, _ b: Int) -> String { String(format: "#%02X%02X%02X", r, g, b) }

/// Per-channel tolerances. Captures are exact BGRA (no compression), so these only
/// absorb colour-conversion rounding and caret anti-aliasing.
let envelopeTolerance = 8
let sameTolerance = 24

func judge(_ frame: ScreenFrame, before: ScreenFrame, final: ScreenFrame, boxes: [PixelBox]) -> Judgement {
    var total = 0, changed = 0, matching = 0, foreign = 0, white = 0
    var foreignSum = (0, 0, 0)
    var bins = [Int](repeating: 0, count: 4096)
    var binSums = [(Int, Int, Int)](repeating: (0, 0, 0), count: 4096)
    let x0 = boxes.map(\.x0).min() ?? 0, x1 = boxes.map(\.x1).max() ?? 0
    let y0 = boxes.map(\.y0).min() ?? 0, y1 = boxes.map(\.y1).max() ?? 0
    let f = frame.pixels, b = before.pixels, s = final.pixels
    guard frame.width == final.width, frame.height == final.height, before.width == final.width,
          before.height == final.height
    else {
        return Judgement(time: frame.time, changed: 0, matching: 0, foreign: 0, white: 0, foreignColor: "-",
                         top: "size changed")
    }
    var y = y0
    while y < y1 {
        var x = x0
        while x < x1 {
            if boxes.contains(where: { x >= $0.x0 && x < $0.x1 && y >= $0.y0 && y < $0.y1 }) {
                let fi = y * frame.stride + x * 4, bi = y * before.stride + x * 4, si = y * final.stride + x * 4
                let fr = Int(f[fi + 2]), fg = Int(f[fi + 1]), fb = Int(f[fi])
                let br = Int(b[bi + 2]), bg = Int(b[bi + 1]), bb = Int(b[bi])
                let sr = Int(s[si + 2]), sg = Int(s[si + 1]), sb = Int(s[si])
                total += 1
                if max(abs(fr - br), abs(fg - bg), abs(fb - bb)) > sameTolerance { changed += 1 }
                if max(abs(fr - sr), abs(fg - sg), abs(fb - sb)) <= sameTolerance { matching += 1 }
                let outside = fr > max(br, sr) + envelopeTolerance || fr < min(br, sr) - envelopeTolerance
                    || fg > max(bg, sg) + envelopeTolerance || fg < min(bg, sg) - envelopeTolerance
                    || fb > max(bb, sb) + envelopeTolerance || fb < min(bb, sb) - envelopeTolerance
                if outside {
                    foreign += 1
                    foreignSum = (foreignSum.0 + fr, foreignSum.1 + fg, foreignSum.2 + fb)
                }
                if fr >= 250 && fg >= 250 && fb >= 250 { white += 1 }
                let bin = (fr >> 4) << 8 | (fg >> 4) << 4 | (fb >> 4)
                bins[bin] += 1
                binSums[bin] = (binSums[bin].0 + fr, binSums[bin].1 + fg, binSums[bin].2 + fb)
            }
            x += 2
        }
        y += 2
    }
    let n = Double(max(total, 1))
    let foreignColor = foreign == 0 ? "-" : hex(foreignSum.0 / foreign, foreignSum.1 / foreign, foreignSum.2 / foreign)
    let top = bins.indices.sorted { bins[$0] > bins[$1] }.prefix(3).filter { bins[$0] > 0 }.map { bin -> String in
        let count = bins[bin]
        return "\(hex(binSums[bin].0 / count, binSums[bin].1 / count, binSums[bin].2 / count)) \(Int((Double(count) / n * 100).rounded()))%"
    }.joined(separator: " ")
    return Judgement(time: frame.time, changed: Double(changed) / n, matching: Double(matching) / n,
                     foreign: Double(foreign) / n, white: Double(white) / n, foreignColor: foreignColor, top: top)
}

func meanLuminance(_ frame: ScreenFrame, boxes: [PixelBox]) -> Double {
    var sum = 0.0, count = 0.0
    for box in boxes {
        var y = box.y0
        while y < box.y1 {
            var x = box.x0
            while x < box.x1 {
                let i = y * frame.stride + x * 4
                sum += 0.2126 * Double(frame.pixels[i + 2]) + 0.7152 * Double(frame.pixels[i + 1])
                    + 0.0722 * Double(frame.pixels[i])
                count += 1
                x += 8
            }
            y += 8
        }
    }
    return count > 0 ? sum / count : 0
}

// MARK: - One launch

struct StartupCase {
    let name: String
    let samples: Int
    /// Global-domain defaults set for the launch: (key, `defaults write` type flag, value).
    var defaults: [(String, String, String)] = []
    var preferences: String? = nil
    /// Two windows saved by Hot Exit, restored by a launch without files.
    var restore = false
    var windows: Int { restore ? 2 : 1 }
}

/// A frame counts as the first frame with content once this much of the window
/// equals the final frame; stable once every later frame equals it this closely.
let contentMatch = 0.90
let stableMatch = 0.995
/// A frame is a flash frame when more of the window than this is foreign.
let flashFrameShare = 0.01

struct LaunchResult {
    let label: String
    let windows: Int
    let frames: Int
    let dropped: Int
    let birth: Double?           // GUI process start, host seconds
    let spawn: Double            // CLI spawn, host seconds
    let timeline: [(String, Double)] // event, host seconds
    let judgements: [Judgement]
    let maxForeign: Double
    let maxForeignAt: Double?
    let flashMs: Double
    let flashFrames: Int
    let firstContent: Double?
    let stable: Double?
    let luminance: Double
    let appMilestones: [(String, Double)] // phase, process ms
    let ownImageNotes: [String]
    let failure: String?

    /// Milliseconds from the GUI process start (or the CLI spawn when unknown).
    func ms(_ host: Double?) -> Double? {
        guard let host else { return nil }
        return (host - (birth ?? spawn)) * 1000
    }
}

func fmt(_ value: Double?) -> String { value.map { String(format: "%.1f", $0) } ?? "-" }
func pct(_ value: Double) -> String { String(format: "%.1f%%", value * 100) }

struct StartupHarness {
    let report: Report
    let environment: Environment
    let recorder: ScreenRecorder
    let owner: String
    let artifacts: String?

    func home(_ name: String) -> URL {
        let url = environment.work.appendingPathComponent("home-\(name)")
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    func run(_ arguments: [String], home: URL, log: String, wait: Bool) -> Process? {
        let process = Process()
        process.executableURL = environment.launcher
        process.arguments = arguments
        var variables = ProcessInfo.processInfo.environment
        variables["HOME"] = home.path
        process.environment = variables
        process.standardOutput = FileHandle.nullDevice
        let logURL = environment.work.appendingPathComponent(log)
        FileManager.default.createFile(atPath: logURL.path, contents: nil)
        process.standardError = (try? FileHandle(forWritingTo: logURL)) ?? FileHandle.nullDevice
        do { try process.run() } catch {
            report.note("launch failed", detail: "\(error)")
            return nil
        }
        if wait {
            let deadline = Date().addingTimeInterval(20)
            while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.02) }
        }
        return process
    }

    func baseArguments(data: URL) -> [String] {
        ["--data-dir", data.path, "--width", "900", "--height", "620"]
    }

    /// Asks the running instance to quit (Hot Exit saves its session), then makes
    /// sure no Velocity process is left.
    func quit(data: URL, home: URL, pids: [pid_t]) {
        _ = run(["--data-dir", data.path, "--command", "exit"], home: home, log: "quit.log", wait: true)
        let gone = waitUntil(15, interval: 0.1) { pids.allSatisfy { !processAlive($0) } }
        if !gone {
            report.note("quit", detail: "instance did not exit after --command exit; killing")
            killVelocity(at: environment.launcher)
        }
        _ = waitUntil(5, interval: 0.1) { velocityWindowRows(owner: owner).isEmpty }
    }

    func setDefaults(_ values: [(String, String, String)]) {
        for (key, type, value) in values {
            _ = shell("defaults write -g \(key.shellQuoted) \(type) \(value.shellQuoted)")
        }
    }

    func clearDefaults(_ values: [(String, String, String)]) {
        for (key, _, _) in values {
            _ = shell("defaults delete -g \(key.shellQuoted) 2>/dev/null || true")
        }
    }

    /// Opens two windows in `data` and quits, so the next launch restores both.
    func prepareRestore(data: URL, home: URL, files: [String]) -> Bool {
        guard run(baseArguments(data: data) + [files[0]], home: home, log: "restore-prepare.log", wait: false) != nil,
              waitUntil(30, interval: 0.1, { velocityWindowRows(owner: owner).contains { $0.onscreen } })
        else { return false }
        sleep(seconds: 1)
        _ = run(["--data-dir", data.path, "-n", files[1]], home: home, log: "restore-second.log", wait: true)
        guard waitUntil(20, interval: 0.1, { velocityWindowRows(owner: owner).filter(\.onscreen).count >= 2 })
        else { return false }
        let pids = Array(Set(velocityWindowRows(owner: owner).map(\.pid)))
        // Side by side, so each restored window is judged on its own pixels.
        if let pid = pids.first {
            let windows = axStandardWindows(pid)
            if windows.count >= 2 {
                _ = moveWindow(windows[0], to: CGRect(x: 20, y: 60, width: 480, height: 520))
                _ = moveWindow(windows[1], to: CGRect(x: 520, y: 60, width: 480, height: 520))
            }
        }
        sleep(seconds: 1.5)
        quit(data: data, home: home, pids: pids)
        return true
    }

    func launch(_ startupCase: StartupCase, sample: Int, files: [String], data: URL, home: URL,
                keepFrames: Bool) -> LaunchResult {
        let label = "\(startupCase.name)-\(sample)"
        let output = environment.work.appendingPathComponent("\(label).jsonl")
        try? FileManager.default.removeItem(at: output)
        recorder.clear()
        recorder.seedWithLatest()
        sleep(seconds: 0.25)
        var arguments = baseArguments(data: data) + ["--output", output.path]
        if !startupCase.restore { arguments += files }
        let spawn = hostNow()
        guard run(arguments, home: home, log: "\(label).stderr", wait: false) != nil else {
            return failed(label, spawn: spawn, "launch failed")
        }
        var seen: [Int: SeenWindow] = [:]
        var allOnscreen: Double?
        let deadline = spawn + 30
        // Window list at ~2 ms; each new window's own image at ~10 ms for its first second.
        var lastOwnImage = 0.0
        while hostNow() < deadline {
            let now = hostNow()
            for row in velocityWindowRows(owner: owner) {
                if seen[row.id] == nil {
                    seen[row.id] = SeenWindow(id: row.id, pid: row.pid, bounds: row.bounds, created: now,
                                              onscreen: nil, alpha: row.alpha)
                }
                seen[row.id]!.bounds = row.bounds
                if row.onscreen && seen[row.id]!.onscreen == nil {
                    seen[row.id]!.onscreen = now
                    seen[row.id]!.alpha = row.alpha
                }
            }
            if now - lastOwnImage > 0.010 {
                lastOwnImage = now
                for (id, window) in seen {
                    if let shown = window.onscreen, now - shown < 1.0, window.ownImages.count < 60,
                       let image = windowOwnImage(id) {
                        if let last = window.ownImages.last, last.sameImage(as: image) { continue }
                        seen[id]!.ownImages.append(image)
                    }
                }
            }
            let shown = seen.values.compactMap(\.onscreen)
            if shown.count >= startupCase.windows {
                allOnscreen = allOnscreen ?? shown.max()
                if let all = allOnscreen, now - all > 2.0 { break }
            }
            Thread.sleep(forTimeInterval: 0.002)
        }
        // A last still frame: the caret may blink, nothing else moves now.
        sleep(seconds: 0.2)
        let taken = recorder.take()
        let windows = Array(seen.values).filter { $0.onscreen != nil }.sorted { $0.created < $1.created }
        let pids = Array(Set(seen.values.map(\.pid)))
        let birth = pids.compactMap(processStartHost).min()
        quit(data: data, home: home, pids: pids)
        let milestones = readMilestones(output)
        guard windows.count >= startupCase.windows else {
            return failed(label, spawn: spawn, "\(windows.count) of \(startupCase.windows) windows on screen",
                          birth: birth, milestones: milestones)
        }
        return analyse(label: label, windows: windows, frames: taken.frames, dropped: taken.dropped, spawn: spawn,
                       birth: birth, milestones: milestones, keepFrames: keepFrames,
                       streamStopped: taken.stopped)
    }

    func failed(_ label: String, spawn: Double, _ why: String, birth: Double? = nil,
                milestones: [(String, Double)] = []) -> LaunchResult {
        LaunchResult(label: label, windows: 0, frames: 0, dropped: 0, birth: birth, spawn: spawn, timeline: [],
                     judgements: [], maxForeign: 0, maxForeignAt: nil, flashMs: 0, flashFrames: 0,
                     firstContent: nil, stable: nil, luminance: 0, appMilestones: milestones, ownImageNotes: [],
                     failure: why)
    }

    func readMilestones(_ url: URL) -> [(String, Double)] {
        guard let text = try? String(contentsOf: url, encoding: .utf8) else { return [] }
        var result: [(String, Double)] = []
        for line in text.split(separator: "\n") {
            guard let data = line.data(using: .utf8),
                  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  object["kind"] as? String == "startup",
                  let milestones = object["milestones"] as? [[String: Any]] else { continue }
            for milestone in milestones {
                if let phase = milestone["phase"] as? String, let ms = milestone["process_ms"] as? Double {
                    result.append((phase, ms))
                }
            }
            if let scanout = object["scanout_process_ms"] as? Double { result.append(("scanout_process", scanout)) }
        }
        return result
    }

    func analyse(label: String, windows: [SeenWindow], frames: [ScreenFrame], dropped: Int, spawn: Double,
                 birth: Double?, milestones: [(String, Double)], keepFrames: Bool,
                 streamStopped: String?) -> LaunchResult {
        guard frames.count >= 2, let before = frames.first, let final = frames.last else {
            return failed(label, spawn: spawn, "only \(frames.count) frames recorded", birth: birth,
                          milestones: milestones)
        }
        let scale = Double(final.width) / Double(CGDisplayBounds(recorder.display).width)
        let boxes = windows.map { window -> PixelBox in
            let r = window.bounds
            return PixelBox(x0: max(0, Int((r.minX * scale).rounded())), y0: max(0, Int((r.minY * scale).rounded())),
                            x1: min(final.width, Int((r.maxX * scale).rounded())),
                            y1: min(final.height, Int((r.maxY * scale).rounded())))
        }
        let firstShown = windows.compactMap(\.onscreen).min() ?? spawn
        // Frames from just before the window came on screen; the first recorded
        // frame (the screen before the launch) is the reference B.
        let judged = frames.dropFirst().filter { $0.time >= spawn }
        let judgements = judged.map { judge($0, before: before, final: final, boxes: boxes) }
        var maxForeign = 0.0, maxForeignAt: Double?
        var flashMs = 0.0, flashFrames = 0
        for (index, judgement) in judgements.enumerated() where judgement.time >= firstShown - 0.1 {
            if judgement.foreign > maxForeign {
                maxForeign = judgement.foreign
                maxForeignAt = judgement.time
            }
            if judgement.foreign > flashFrameShare {
                flashFrames += 1
                let next = index + 1 < judgements.count ? judgements[index + 1].time : judgement.time + 1.0 / 60
                flashMs += (next - judgement.time) * 1000
            }
        }
        let firstPixels = judgements.first { $0.changed > 0.02 }?.time
        let firstContent = judgements.first { $0.matching >= contentMatch }?.time
        var stable: Double?
        for judgement in judgements.reversed() {
            if judgement.matching >= stableMatch { stable = judgement.time } else { break }
        }
        var timeline: [(String, Double)] = [("cli_spawn", spawn)]
        for (index, window) in windows.enumerated() {
            timeline.append(("window\(index + 1)_listed", window.created))
            if let shown = window.onscreen { timeline.append(("window\(index + 1)_onscreen", shown)) }
        }
        if let firstPixels { timeline.append(("screen_first_window_pixels", firstPixels)) }
        if let maxForeignAt, maxForeign > flashFrameShare { timeline.append(("screen_max_foreign", maxForeignAt)) }
        if let firstContent { timeline.append(("screen_first_content", firstContent)) }
        if let stable { timeline.append(("screen_stable", stable)) }

        // The window's own backing images: what the window server held before the
        // screen showed content (an empty window shows its background here).
        var ownNotes: [String] = []
        for (index, window) in windows.enumerated() {
            guard let last = window.ownImages.last else {
                ownNotes.append("window\(index + 1): no own image")
                continue
            }
            let box = [PixelBox(x0: 0, y0: 0, x1: last.width, y1: last.height)]
            let sequence = window.ownImages.map { image -> String in
                guard image.width == last.width, image.height == last.height else {
                    return "+\(fmt(((image.time - (birth ?? spawn)) * 1000))) size \(image.width)x\(image.height)"
                }
                let j = judge(image, before: last, final: last, boxes: box)
                let t = fmt((image.time - (birth ?? spawn)) * 1000)
                return "+\(t)ms white=\(pct(j.white)) top=\(j.top)"
            }
            ownNotes.append("window\(index + 1) own images: " + sequence.prefix(8).joined(separator: " | "))
            if keepFrames, let dir = artifacts, let first = window.ownImages.first {
                writePNG(first, crop: nil, to: "\(dir)/startup/\(label)-window\(index + 1)-own-first.png")
            }
        }

        if keepFrames, let dir = artifacts {
            let crop = CGRect(x: boxes.map(\.x0).min()!, y: boxes.map(\.y0).min()!,
                              width: boxes.map(\.x1).max()! - boxes.map(\.x0).min()!,
                              height: boxes.map(\.y1).max()! - boxes.map(\.y0).min()!)
            // Every frame from the window's appearance until it is stable, then the final one.
            var saved = 0
            for (index, frame) in judged.enumerated() where frame.time >= firstShown - 0.05 {
                if let stable, frame.time > stable { break }
                if saved >= 24 { break }
                let t = Int((((frame.time - (birth ?? spawn)) * 1000)).rounded())
                writePNG(frame, crop: crop, to: String(format: "%@/startup/%@-f%02d-t%05dms.png", dir, label, index, t))
                saved += 1
            }
            writePNG(final, crop: crop, to: "\(dir)/startup/\(label)-final.png")
        }
        let luminance = meanLuminance(final, boxes: boxes)
        var failure: String?
        if let streamStopped { failure = "capture stopped: \(streamStopped)" }
        return LaunchResult(label: label, windows: windows.count, frames: frames.count, dropped: dropped,
                            birth: birth, spawn: spawn, timeline: timeline, judgements: judgements,
                            maxForeign: maxForeign, maxForeignAt: maxForeignAt, flashMs: flashMs,
                            flashFrames: flashFrames, firstContent: firstContent, stable: stable,
                            luminance: luminance, appMilestones: milestones, ownImageNotes: ownNotes,
                            failure: failure)
    }

    func describe(_ result: LaunchResult) {
        if let failure = result.failure { report.note("\(result.label) problem", detail: failure) }
        let origin = result.birth != nil ? "GUI process start" : "CLI spawn"
        report.note("\(result.label) summary", detail: "windows=\(result.windows) frames=\(result.frames) dropped=\(result.dropped) max_foreign=\(pct(result.maxForeign)) flash_frames=\(result.flashFrames) flash_ms=\(fmt(result.flashMs)) first_content=\(fmt(result.ms(result.firstContent)))ms stable=\(fmt(result.ms(result.stable)))ms luminance=\(Int(result.luminance)) (times from \(origin))")
        let events = result.timeline.map { "\($0.0)=\(fmt(result.ms($0.1)))" }.joined(separator: " ")
        report.note("\(result.label) timeline ms", detail: events)
        let interesting = ["window_layer_ready", "first_frame_prepared_before_order_front", "ordered_front",
                           "launch_callback_finished", "first_visible_display_tick", "first_submit",
                           "first_presented_callback", "first_real_scanout", "scanout_process"]
        let app = result.appMilestones.filter { interesting.contains($0.0) }
            .map { "\($0.0)=\(fmt($0.1))" }.joined(separator: " ")
        report.note("\(result.label) app milestones (process ms)", detail: app.isEmpty ? "none" : app)
        let firstShown = result.timeline.first { $0.0.hasSuffix("_onscreen") }?.1 ?? result.spawn
        let frames = result.judgements.filter { $0.time >= firstShown - 0.05 }.prefix(14).map { j in
            "+\(fmt(result.ms(j.time))) chg=\(pct(j.changed)) eq=\(pct(j.matching)) foreign=\(pct(j.foreign))\(j.foreign > 0.001 ? "(\(j.foreignColor))" : "") white=\(pct(j.white)) [\(j.top)]"
        }
        report.note("\(result.label) frames", detail: "\n    " + frames.joined(separator: "\n    "))
        for note in result.ownImageNotes { report.note("\(result.label) own", detail: note) }
        // One machine-readable line per launch, for before/after comparisons.
        var json: [String: Any] = ["label": result.label, "windows": result.windows,
                                   "max_foreign": result.maxForeign, "flash_ms": result.flashMs,
                                   "flash_frames": result.flashFrames, "luminance": result.luminance]
        for (name, host) in result.timeline { json[name] = result.ms(host) }
        for (name, ms) in result.appMilestones { json["app_" + name] = ms }
        if let data = try? JSONSerialization.data(withJSONObject: json, options: [.sortedKeys]),
           let text = String(data: data, encoding: .utf8) {
            print("STARTUP_JSON \(text)")
            fflush(stdout)
        }
    }
}

func median(_ values: [Double]) -> Double? {
    let sorted = values.sorted()
    guard !sorted.isEmpty else { return nil }
    return sorted.count % 2 == 1 ? sorted[sorted.count / 2]
        : (sorted[sorted.count / 2 - 1] + sorted[sorted.count / 2]) / 2
}

// MARK: - Scenario

/// Judged ceilings. Foreign share per frame: a flash of the empty window covers
/// nearly the whole content area; the caret and the edges of a scale animation
/// stay far below 2 %.
let maxForeignShare = 0.02
/// Median first-content time per case may not regress past the pre-fix median by
/// more than this (CI noise on a shared 3-core runner), when a baseline is given.
let regressionAllowanceMs = 40.0
let regressionAllowanceShare = 0.25

func scenarioStartup(_ options: ArraySlice<String>) -> Never {
    let report = Report("startup")
    let environment = Environment.from(options)
    environment.reset()
    dismissAlerts(report: report)
    let owner = URL(fileURLWithPath: environment.executable).lastPathComponent
    let recorder = ScreenRecorder()
    guard recorder.start() else {
        report.check(false, "screen recording starts", detail: recorder.failure)
        report.finish()
    }
    report.check(true, "screen recording starts", detail: "\(recorder.method); scale \(recorder.scale); refresh \(recorder.refreshRate) Hz")
    let harness = StartupHarness(report: report, environment: environment, recorder: recorder, owner: owner,
                                 artifacts: ProcessInfo.processInfo.environment["E2E_ARTIFACTS"])
    var body = "// Startup fixture: enough text to fill the window.\n"
    for line in 1...120 {
        body += "fn item_\(line)(value: usize) -> usize { value.wrapping_mul(\(line)).rotate_left(\(line % 31)) }\n"
    }
    let files = [environment.fixture("startup.rs", contents: body),
                 environment.fixture("second.txt", contents: "The second restored window.\n")]
    let darkScheme = "{\n  \"color_scheme\": \"Velocity Dark.sublime-color-scheme\",\n  \"theme\": \"Velocity.sublime-theme\"\n}\n"
    let only = ProcessInfo.processInfo.environment["E2E_STARTUP_CASES"].map { Set($0.split(separator: ",").map(String.init)) }
    let cases = [
        StartupCase(name: "light", samples: 3),
        StartupCase(name: "light-noanim", samples: 2,
                    defaults: [("NSAutomaticWindowAnimationsEnabled", "-bool", "false")]),
        StartupCase(name: "dark", samples: 3, defaults: [("AppleInterfaceStyle", "-string", "Dark")]),
        StartupCase(name: "dark-scheme-light-system", samples: 2, preferences: darkScheme),
        StartupCase(name: "restore2", samples: 2, restore: true),
    ].filter { only?.contains($0.name) ?? true }
    // Baseline first-content medians (ms from GUI process start) measured before the
    // fix, "light=123,dark=130"; without them speed is only reported.
    var baseline: [String: Double] = [:]
    for pair in (ProcessInfo.processInfo.environment["E2E_STARTUP_BASELINE"] ?? "").split(separator: ",") {
        let parts = pair.split(separator: "=")
        if parts.count == 2, let value = Double(parts[1]) { baseline[String(parts[0])] = value }
    }

    // Warm-up: page cache, shader cache and font cache, so the first judged launch
    // is not the only cold one.
    do {
        let data = environment.work.appendingPathComponent("data-warmup")
        let home = harness.home("warmup")
        _ = harness.launch(StartupCase(name: "warmup", samples: 1), sample: 0, files: [files[0]], data: data,
                           home: home, keepFrames: false)
    }

    var medians: [String: Double] = [:]
    for startupCase in cases {
        harness.setDefaults(startupCase.defaults)
        defer { harness.clearDefaults(startupCase.defaults) }
        let home = harness.home(startupCase.name)
        var results: [LaunchResult] = []
        let restoreData = environment.work.appendingPathComponent("data-\(startupCase.name)")
        if startupCase.restore {
            let prepared = harness.prepareRestore(data: restoreData, home: home, files: files)
            report.check(prepared, "\(startupCase.name): two windows opened and saved by quitting")
            if !prepared { continue }
        }
        for sample in 1...startupCase.samples {
            let data = startupCase.restore ? restoreData
                : environment.work.appendingPathComponent("data-\(startupCase.name)-\(sample)")
            if let preferences = startupCase.preferences {
                let user = data.appendingPathComponent("Packages/User")
                try? FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
                try? preferences.write(to: user.appendingPathComponent("Preferences.sublime-settings"),
                                       atomically: true, encoding: .utf8)
            }
            dismissAlerts(report: report)
            let result = harness.launch(startupCase, sample: sample, files: [files[0]], data: data, home: home,
                                        keepFrames: true)
            harness.describe(result)
            results.append(result)
        }
        let judged = results.filter { $0.windows > 0 }
        report.check(judged.count == startupCase.samples && judged.allSatisfy { $0.windows == startupCase.windows },
                     "\(startupCase.name): every launch shows \(startupCase.windows) window(s) and is recorded",
                     detail: results.map { "\($0.label): \($0.failure ?? "ok")" }.joined(separator: "; "))
        let worst = judged.max { $0.maxForeign < $1.maxForeign }
        report.check(judged.allSatisfy { $0.maxForeign <= maxForeignShare },
                     "\(startupCase.name): no frame shows more than \(pct(maxForeignShare)) of the window in a colour that is neither the screen before launch nor the final frame",
                     detail: worst.map { "worst \($0.label): \(pct($0.maxForeign)) over \($0.flashFrames) frame(s), \(fmt($0.flashMs)) ms" } ?? "no launch")
        if startupCase.name.hasPrefix("dark") {
            let dark = judged.allSatisfy { $0.luminance < 110 }
            report.check(dark, "\(startupCase.name): the final window is dark",
                         detail: judged.map { "\($0.label) luminance \(Int($0.luminance))" }.joined(separator: ", "))
        }
        let contents = judged.compactMap { $0.ms($0.firstContent) }
        if let m = median(contents) {
            medians[startupCase.name] = m
            let scanouts = judged.compactMap { result in result.appMilestones.first { $0.0 == "first_real_scanout" }?.1 }
            report.info("\(startupCase.name): first content median \(fmt(m)) ms from process start",
                        detail: "samples \(contents.map { fmt($0) }.joined(separator: ", ")); app first_real_scanout median \(fmt(median(scanouts))) ms")
            if let before = baseline[startupCase.name] {
                let ceiling = before + max(regressionAllowanceMs, before * regressionAllowanceShare)
                report.check(m <= ceiling, "\(startupCase.name): first content is not slower than before the fix",
                             detail: "median \(fmt(m)) ms, before \(fmt(before)) ms, ceiling \(fmt(ceiling)) ms")
            }
        }
    }
    report.info("first content medians", detail: medians.sorted { $0.key < $1.key }
        .map { "\($0.key)=\(fmt($0.value))" }.joined(separator: ","))
    recorder.stop()
    killVelocity(at: environment.launcher)
    report.finish()
}
