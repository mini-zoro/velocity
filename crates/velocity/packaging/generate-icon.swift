// Original Velocity artwork, MIT OR Apache-2.0. No external images, fonts or artwork.
// A folded cyan V and two motion bars on an indigo rounded square.
import AppKit

guard CommandLine.arguments.count == 2 else {
    fatalError("usage: generate-icon.swift OUTPUT.iconset")
}
let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = points * scale
        let bitmap = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
            isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        let context = NSGraphicsContext(bitmapImageRep: bitmap)!
        NSGraphicsContext.current = context
        context.shouldAntialias = true
        context.cgContext.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)
        let background = NSBezierPath(roundedRect: NSRect(x: 48, y: 48, width: 928, height: 928), xRadius: 212, yRadius: 212)
        NSGradient(starting: NSColor(srgbRed: 0.07, green: 0.10, blue: 0.23, alpha: 1),
                   ending: NSColor(srgbRed: 0.18, green: 0.23, blue: 0.46, alpha: 1))!
            .draw(in: background, angle: 70)
        func polygon(_ points: [(CGFloat, CGFloat)], _ color: NSColor) {
            let path = NSBezierPath()
            path.move(to: NSPoint(x: points[0].0, y: points[0].1))
            for point in points.dropFirst() { path.line(to: NSPoint(x: point.0, y: point.1)) }
            path.close()
            color.setFill()
            path.fill()
        }
        polygon([(223, 706), (365, 706), (535, 306), (437, 230)],
                NSColor(srgbRed: 0.32, green: 0.76, blue: 0.93, alpha: 1))
        polygon([(437, 230), (535, 306), (811, 794), (650, 794)],
                NSColor(srgbRed: 0.64, green: 0.98, blue: 0.95, alpha: 1))
        polygon([(201, 548), (309, 548), (337, 482), (173, 482)],
                NSColor(srgbRed: 0.42, green: 0.59, blue: 0.86, alpha: 1))
        polygon([(167, 423), (362, 423), (389, 357), (139, 357)],
                NSColor(srgbRed: 0.42, green: 0.59, blue: 0.86, alpha: 1))
        NSGraphicsContext.restoreGraphicsState()
        let suffix = scale == 2 ? "@2x" : ""
        let name = "icon_\(points)x\(points)\(suffix).png"
        try bitmap.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent(name))
    }
}
