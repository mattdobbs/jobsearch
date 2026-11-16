// Draws the JobSearch app icon: a white briefcase on a blue rounded tile, matching
// the briefcase in the menu bar.
//
// Usage: icon <output.png>      (writes a 1024 x 1024 PNG)
//
// Run through scripts/make-icon.sh, which turns the PNG into assets/AppIcon.icns.

import CoreGraphics
import Foundation
import ImageIO

guard CommandLine.arguments.count == 2 else {
    FileHandle.standardError.write(Data("usage: icon <output.png>\n".utf8))
    exit(2)
}

let side = 1024
let space = CGColorSpace(name: CGColorSpace.sRGB)!
guard let ctx = CGContext(
    data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
else {
    FileHandle.standardError.write(Data("could not create a drawing surface\n".utf8))
    exit(1)
}

func rgb(_ hex: UInt32, alpha: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: space, components: [
        CGFloat((hex >> 16) & 0xFF) / 255,
        CGFloat((hex >> 8) & 0xFF) / 255,
        CGFloat(hex & 0xFF) / 255,
        alpha,
    ])!
}

func rounded(_ rect: CGRect, _ radius: CGFloat) -> CGPath {
    CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil)
}

// The tile follows the macOS app-icon grid: an 824-point square centred in the
// 1024-point canvas, leaving room for the shadow.
let tile = rounded(CGRect(x: 100, y: 100, width: 824, height: 824), 185)

ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 22, color: rgb(0x000000, alpha: 0.30))
ctx.addPath(tile)
ctx.setFillColor(rgb(0x1F4FA8))
ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(tile)
ctx.clip()
let gradient = CGGradient(
    colorsSpace: space, colors: [rgb(0x4C93E6), rgb(0x1B4699)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(
    gradient, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])
ctx.restoreGState()

// The briefcase is drawn in its own layer so the seam can be cut out of it, letting
// the tile show through.
let white = rgb(0xFFFFFF)
let seamY: CGFloat = 474
ctx.beginTransparencyLayer(auxiliaryInfo: nil)

ctx.setStrokeColor(white)
ctx.setLineWidth(46)
ctx.addPath(rounded(CGRect(x: 402, y: 590, width: 220, height: 150), 44))  // handle
ctx.strokePath()

ctx.setFillColor(white)
ctx.addPath(rounded(CGRect(x: 242, y: 270, width: 540, height: 360), 60))  // body
ctx.fillPath()

ctx.setBlendMode(.clear)
ctx.fill(CGRect(x: 242, y: seamY - 12, width: 540, height: 24))  // seam
ctx.addPath(rounded(CGRect(x: 512 - 62, y: seamY - 52, width: 124, height: 104), 22))  // around the clasp
ctx.fillPath()

ctx.setBlendMode(.normal)
ctx.setFillColor(white)
ctx.addPath(rounded(CGRect(x: 512 - 38, y: seamY - 28, width: 76, height: 56), 12))  // clasp
ctx.fillPath()

ctx.endTransparencyLayer()

guard let image = ctx.makeImage(),
    let destination = CGImageDestinationCreateWithURL(
        URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL, "public.png" as CFString, 1, nil)
else {
    FileHandle.standardError.write(Data("could not write \(CommandLine.arguments[1])\n".utf8))
    exit(1)
}
CGImageDestinationAddImage(destination, image, nil)
if !CGImageDestinationFinalize(destination) {
    FileHandle.standardError.write(Data("could not write \(CommandLine.arguments[1])\n".utf8))
    exit(1)
}
