// macOS-only reference: renders every page with PDFKit + Core Graphics into a BGRA buffer.
// Usage: swiftc -O pdfkit-bench.swift -o ../target/pdfkit-bench && ../target/pdfkit-bench <file.pdf> [width] [search]
import CoreGraphics
import ImageIO
import Foundation
import PDFKit

let args = CommandLine.arguments
let path = args[1]
let width = args.count > 2 ? Int(args[2])! : 1600
let needle = args.count > 3 ? args[3] : "tax"

var t = Date()
guard let doc = PDFDocument(url: URL(fileURLWithPath: path)) else { fatalError("open failed: \(path)") }
let count = doc.pageCount
let openMs = Date().timeIntervalSince(t) * 1000

var times: [Double] = []
var cache: [Data] = []
var peak = 0
let space = CGColorSpaceCreateDeviceRGB()
for i in 0..<count { autoreleasepool {
  t = Date()
  let page = doc.page(at: i)!
  let box = page.bounds(for: .mediaBox)
  let scale = CGFloat(width) / box.width
  let h = Int((box.height * scale).rounded())
  // premultipliedFirst + byteOrder32Little is BGRA in memory, GPUI's RenderImage layout.
  let ctx = CGContext(data: nil, width: width, height: h, bitsPerComponent: 8, bytesPerRow: width * 4, space: space,
                      bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)!
  ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
  ctx.fill(CGRect(x: 0, y: 0, width: width, height: h))
  ctx.scaleBy(x: scale, y: scale)
  page.draw(with: .mediaBox, to: ctx)
  let data = Data(bytes: ctx.data!, count: width * h * 4)
  if args.count > 4, i < 2, let image = ctx.makeImage() {
    let name = URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent
    let url = URL(fileURLWithPath: "\(args[4])/pdfkit-\(name)-p\(i + 1).png") as CFURL
    let dest = CGImageDestinationCreateWithURL(url, "public.png" as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, image, nil)
    CGImageDestinationFinalize(dest)
  }
  times.append(Date().timeIntervalSince(t) * 1000)
  cache.append(data)
  if cache.count > 6 { cache.removeFirst() }
  peak = max(peak, cache.reduce(0) { $0 + $1.count })
} // autoreleasepool: release per-page PDFKit objects as a viewer would
}
t = Date()
var chars = 0
for i in 0..<count { chars += doc.page(at: i)?.string?.count ?? 0 }
let hits = doc.findString(needle, withOptions: .caseInsensitive).count
let textMs = Date().timeIntervalSince(t) * 1000
let sorted = times.sorted()
let total = times.reduce(0, +)
print(String(format: "{\"renderer\":\"pdfkit\",\"file\":\"%@\",\"width\":%d,\"page_count\":%d,\"open_ms\":%.1f,\"pages\":%d,\"total_ms\":%.1f,\"mean_ms\":%.2f,\"p50_ms\":%.2f,\"p95_ms\":%.2f,\"max_ms\":%.2f,\"cache_pages\":6,\"cache_peak_mb\":%.1f,\"text_chars\":%d,\"search\":\"%@\",\"search_hits_case_insensitive\":%d,\"text_and_search_ms\":%.1f}",
  path, width, count, openMs, count, total, total / Double(count), sorted[count / 2], sorted[min(count - 1, count * 95 / 100)], sorted[count - 1],
  Double(peak) / 1048576, chars, needle, hits, textMs))
