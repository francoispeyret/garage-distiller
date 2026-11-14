#!/usr/bin/env python3
"""Regenerate Garage Distiller icons on macOS using AppKit (no Python packages).

Run: python3 scripts/generate_icon.py
Produces assets/icon.png (1024 px), icon.icns and a multi-resolution icon.ico.
Requires the macOS command line developer tools. ICO and ICNS containers are
written directly with the Python standard library.
"""

from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"

SWIFT = r'''
import AppKit

let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let dark = NSColor(srgbRed: 12.0/255, green: 15.0/255, blue: 20.0/255, alpha: 1)
let lime = NSColor(srgbRed: 199.0/255, green: 244.0/255, blue: 113.0/255, alpha: 1)

func render(_ pixels: Int) throws {
    let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
        isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: pixels * 4, bitsPerPixel: 32
    )!
    bitmap.size = NSSize(width: pixels, height: pixels)
    NSGraphicsContext.saveGraphicsState()
    let context = NSGraphicsContext(bitmapImageRep: bitmap)!
    NSGraphicsContext.current = context
    context.shouldAntialias = true
    context.imageInterpolation = .high
    let scale = CGFloat(pixels) / 1024
    context.cgContext.scaleBy(x: scale, y: scale)

    dark.setFill()
    NSBezierPath(roundedRect: NSRect(x: 24, y: 24, width: 976, height: 976),
                 xRadius: 224, yRadius: 224).fill()
    lime.setFill()
    NSBezierPath(roundedRect: NSRect(x: 100, y: 100, width: 824, height: 824),
                 xRadius: 176, yRadius: 176).fill()

    let system = NSFont.systemFont(ofSize: 552, weight: .bold)
    let rounded = system.fontDescriptor.withDesign(.rounded) ?? system.fontDescriptor
    let font = NSFont(descriptor: rounded, size: 552) ?? system
    let text = NSAttributedString(string: "gd", attributes: [
        .font: font, .foregroundColor: dark, .kern: -32,
    ])
    let size = text.size()
    // Correct typographic side bearings and baseline for optical centering.
    text.draw(at: NSPoint(x: (1024 - size.width) / 2 - 9,
                         y: (1024 - size.height) / 2 + 65))
    NSGraphicsContext.restoreGraphicsState()
    let data = bitmap.representation(using: .png, properties: [:])!
    try data.write(to: output.appendingPathComponent("icon-\(pixels).png"))
}

for pixels in [16, 32, 48, 64, 128, 256, 512, 1024] {
    try render(pixels)
}
'''


def make_ico(images: list[tuple[int, bytes]]) -> bytes:
    """ICO directory followed by PNG payloads, supported by modern Windows."""
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    directory = bytearray()
    payload = bytearray()
    for size, png in images:
        dimension = 0 if size == 256 else size
        directory.extend(struct.pack("<BBBBHHII", dimension, dimension, 0, 0,
                                     1, 32, len(png), offset))
        payload.extend(png)
        offset += len(png)
    return header + directory + payload


def make_icns(images: dict[int, bytes]) -> bytes:
    """Modern ICNS chunks contain PNG data; each length includes its 8-byte header."""
    types = {16: b"icp4", 32: b"icp5", 64: b"icp6", 128: b"ic07",
             256: b"ic08", 512: b"ic09", 1024: b"ic10"}
    chunks = bytearray()
    for size, kind in types.items():
        png = images[size]
        chunks.extend(kind + struct.pack(">I", len(png) + 8) + png)
    return b"icns" + struct.pack(">I", len(chunks) + 8) + chunks


def main() -> None:
    if sys.platform != "darwin":
        raise SystemExit("La génération utilise AppKit : exécutez ce script sur macOS.")
    ASSETS.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="garage-distiller-icons-") as scratch:
        temporary = Path(scratch)
        source = temporary / "render.swift"
        source.write_text(SWIFT, encoding="utf-8")
        subprocess.run(["xcrun", "swift", "-module-cache-path", str(temporary / "cache"),
                        str(source), str(temporary)], check=True)
        shutil.copyfile(temporary / "icon-1024.png", ASSETS / "icon.png")

        icns_images = {size: (temporary / f"icon-{size}.png").read_bytes()
                       for size in (16, 32, 64, 128, 256, 512, 1024)}
        (ASSETS / "icon.icns").write_bytes(make_icns(icns_images))
        images = [(size, (temporary / f"icon-{size}.png").read_bytes())
                  for size in (16, 32, 48, 64, 128, 256)]
        (ASSETS / "icon.ico").write_bytes(make_ico(images))
    print("Icônes générées : assets/icon.png, assets/icon.icns, assets/icon.ico")


if __name__ == "__main__":
    main()
