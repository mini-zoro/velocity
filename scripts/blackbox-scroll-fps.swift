// Black-box scroll frame rate with ScreenCaptureKit: capture one window, post scroll-wheel
// events to its app at 120 Hz, count frames in which the window content changed.
// Usage: scrollfps <pid> <seconds> [lines-per-event]
import AppKit
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

let args = CommandLine.arguments
guard args.count >= 3, let pid = pid_t(args[1]), let seconds = Double(args[2]) else {
    print("usage: scrollfps <pid> <seconds> [lines]"); exit(2)
}
let lines = args.count > 3 ? Int32(args[3])! : -3
// ScreenCaptureKit and CGEvent need a WindowServer connection (CGS) in a command-line tool.
let application = NSApplication.shared
application.setActivationPolicy(.prohibited)
_ = CGMainDisplayID()

final class Counter: NSObject, SCStreamOutput {
    let lock = NSLock()
    var recording = false
    var times: [Double] = []
    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen,
              let info = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let raw = info.first?[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete else { return }
        lock.lock(); if recording { times.append(CMSampleBufferGetPresentationTimeStamp(sb).seconds) }; lock.unlock()
    }
}

let done = DispatchSemaphore(value: 0)
Task {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true)
        guard let window = content.windows.first(where: { $0.owningApplication?.processID == pid && $0.windowLayer == 0 && $0.frame.width > 200 }) else {
            print("no window"); exit(1)
        }
        let filter = SCContentFilter(desktopIndependentWindow: window)
        let config = SCStreamConfiguration()
        let scale = NSScreen.screens.first(where: { $0.frame.intersects(window.frame) })?.backingScaleFactor ?? 2
        config.width = Int(window.frame.width * scale); config.height = Int(window.frame.height * scale)
        config.minimumFrameInterval = CMTime(value: 1, timescale: 240)
        config.queueDepth = 8
        config.showsCursor = false
        let counter = Counter()
        let stream = SCStream(filter: filter, configuration: config, delegate: nil)
        try stream.addStreamOutput(counter, type: .screen, sampleHandlerQueue: DispatchQueue(label: "frames"))
        try await stream.startCapture()
        let src = CGEventSource(stateID: .hidSystemState)
        // CG global coordinates (top-left origin) of the window centre.
        let centre = CGPoint(x: window.frame.midX, y: window.frame.midY)
        func scroll() {
            let e = CGEvent(scrollWheelEvent2Source: src, units: .line, wheelCount: 1, wheel1: lines, wheel2: 0, wheel3: 0)!
            e.location = centre
            e.postToPid(pid)
        }
        for _ in 0..<30 { scroll(); usleep(8333) }
        usleep(400_000)
        counter.lock.lock(); counter.times.removeAll(); counter.recording = true; counter.lock.unlock()
        let end = Date().addingTimeInterval(seconds)
        var posted = 0
        while Date() < end { scroll(); posted += 1; usleep(8333) }
        usleep(200_000)
        counter.lock.lock(); counter.recording = false; let t = counter.times; counter.lock.unlock()
        try await stream.stopCapture()
        let gaps = zip(t.dropFirst(), t).map { ($0 - $1) * 1000 }.sorted()
        let fps = t.count > 1 ? Double(t.count - 1) / (t.last! - t.first!) : 0
        func q(_ p: Double) -> Double { gaps.isEmpty ? -1 : gaps[min(gaps.count - 1, Int(Double(gaps.count) * p))] }
        print(String(format: "frames=%d fps=%.1f gap_p50=%.1fms gap_p95=%.1fms gap_max=%.1fms posted=%d window=%.0fx%.0f@%.0fx",
                     t.count, fps, q(0.5), q(0.95), gaps.last ?? -1, posted, window.frame.width, window.frame.height, scale))
    } catch { print("error: \(error)") }
    done.signal()
}
done.wait()
