import AppKit
import CoreGraphics
import Foundation

guard CommandLine.arguments.count == 2 else {
    fputs("usage: generate-status-icon.swift <output.pdf>\n", stderr)
    exit(2)
}

let outputURL = URL(fileURLWithPath: CommandLine.arguments[1])
var mediaBox = CGRect(x: 0, y: 0, width: 22, height: 22)

guard let consumer = CGDataConsumer(url: outputURL as CFURL),
      let context = CGContext(consumer: consumer, mediaBox: &mediaBox, nil) else {
    fputs("failed to create PDF context\n", stderr)
    exit(1)
}

context.beginPDFPage(nil)
context.translateBy(x: 0, y: 22)
context.scaleBy(x: 0.5, y: -0.5)

context.setStrokeColor(NSColor.black.cgColor)
context.setFillColor(NSColor.black.cgColor)
context.setLineCap(.round)
context.setLineJoin(.round)

context.setLineWidth(3.6)
let body = CGPath(
    roundedRect: CGRect(x: 3.5, y: 8.5, width: 37, height: 27),
    cornerWidth: 7.5,
    cornerHeight: 7.5,
    transform: nil
)
context.addPath(body)
context.strokePath()

context.setLineWidth(2.8)
let pogo = CGPath(
    roundedRect: CGRect(x: 12, y: 12.6, width: 20, height: 9.4),
    cornerWidth: 4.7,
    cornerHeight: 4.7,
    transform: nil
)
context.addPath(pogo)
context.strokePath()

for centerX in [17.2, 22.0, 26.8] {
    context.fillEllipse(in: CGRect(x: centerX - 1.6, y: 17.3 - 1.6, width: 3.2, height: 3.2))
}

context.endPDFPage()
context.closePDF()
