// Renders the Mori app icon to a 1024×1024 PNG: swift scripts/make-icon.swift <out.png>
import AppKit

let size: CGFloat = 1024
let output = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "AppIcon.png"

let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

// macOS icon grid: 824pt body centred in the 1024 canvas.
let body = NSRect(x: 100, y: 100, width: 824, height: 824)
let squircle = NSBezierPath(roundedRect: body, xRadius: 185, yRadius: 185)

NSGraphicsContext.saveGraphicsState()
let shadow = NSShadow()
shadow.shadowColor = NSColor.black.withAlphaComponent(0.22)
shadow.shadowBlurRadius = 28
shadow.shadowOffset = NSSize(width: 0, height: -12)
shadow.set()
NSColor.white.setFill()
squircle.fill()
NSGraphicsContext.restoreGraphicsState()

let gradient = NSGradient(colors: [
    NSColor(calibratedRed: 0.22, green: 0.48, blue: 0.36, alpha: 1),
    NSColor(calibratedRed: 0.07, green: 0.28, blue: 0.22, alpha: 1),
])!
gradient.draw(in: squircle, angle: -90)

// One cream leaf, tilted, pointed at both ends.
let leaf = NSBezierPath()
leaf.move(to: NSPoint(x: 455, y: 275))
leaf.curve(
    to: NSPoint(x: 590, y: 755),
    controlPoint1: NSPoint(x: 230, y: 390),
    controlPoint2: NSPoint(x: 310, y: 690)
)
leaf.curve(
    to: NSPoint(x: 455, y: 275),
    controlPoint1: NSPoint(x: 820, y: 620),
    controlPoint2: NSPoint(x: 700, y: 340)
)
leaf.close()
NSColor(calibratedRed: 0.96, green: 0.94, blue: 0.88, alpha: 1).setFill()
leaf.fill()

let vein = NSBezierPath()
vein.move(to: NSPoint(x: 478, y: 330))
vein.curve(
    to: NSPoint(x: 568, y: 700),
    controlPoint1: NSPoint(x: 500, y: 470),
    controlPoint2: NSPoint(x: 530, y: 590)
)
vein.lineWidth = 16
vein.lineCapStyle = .round
NSColor(calibratedRed: 0.13, green: 0.36, blue: 0.27, alpha: 0.45).setStroke()
vein.stroke()

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output))
print("wrote \(output)")
