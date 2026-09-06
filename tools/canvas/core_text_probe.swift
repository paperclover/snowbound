import CoreText
import Foundation

struct Run: Codable {
    let text: String
    let font: String
    let size: Double
    let bold: Bool
    let italic: Bool
}

struct Case: Decodable {
    let id: String
    let width: Double
    let runs: [Run]
}

guard CommandLine.arguments.count >= 2 else {
    fatalError("Expected a text-case JSON file and optional font files")
}
for path in CommandLine.arguments.dropFirst(2) {
    var error: Unmanaged<CFError>?
    guard CTFontManagerRegisterFontsForURL(URL(fileURLWithPath: path) as CFURL, .process, &error) else {
        throw error!.takeRetainedValue()
    }
}
let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
let cases = try JSONDecoder().decode([Case].self, from: data)
var results: [[String: Any]] = []
for item in cases {
    precondition(item.width.isFinite && item.width > 0 && !item.runs.isEmpty)
    let text = NSMutableAttributedString(string: "")
    var emptyFont: CTFont?
    var resolvedFonts: [String] = []
    for run in item.runs {
        precondition(run.size.isFinite && run.size > 0)
        let base = CTFontCreateWithName(run.font as CFString, run.size, nil)
        var traits: CTFontSymbolicTraits = []
        if run.bold { traits.insert(.boldTrait) }
        if run.italic { traits.insert(.italicTrait) }
        let font = CTFontCreateCopyWithSymbolicTraits(base, run.size, nil, traits, [.boldTrait, .italicTrait]) ?? base
        emptyFont = emptyFont ?? font
        resolvedFonts.append(CTFontCopyPostScriptName(font) as String)
        text.append(NSAttributedString(string: run.text, attributes: [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
        ]))
    }
    let typesetter = CTTypesetterCreateWithAttributedString(text)
    var start = 0
    var lines: [[String: Any]] = []
    repeat {
        let count = text.length == 0 ? 0 : CTTypesetterSuggestLineBreak(typesetter, start, item.width)
        precondition(count > 0 || text.length == 0, "Typesetter made no progress")
        let line = CTTypesetterCreateLine(typesetter, CFRange(location: start, length: count))
        var ascent: CGFloat = 0
        var descent: CGFloat = 0
        var leading: CGFloat = 0
        let advance = CTLineGetTypographicBounds(line, &ascent, &descent, &leading)
        if text.length == 0 {
            ascent = CTFontGetAscent(emptyFont!)
            descent = CTFontGetDescent(emptyFont!)
            leading = CTFontGetLeading(emptyFont!)
        }
        let faces = (CTLineGetGlyphRuns(line) as! [CTRun]).map { run -> [String: Any] in
            let attributes = CTRunGetAttributes(run) as NSDictionary
            let font = attributes[kCTFontAttributeName] as! CTFont
            var result: [String: Any] = ["face": CTFontCopyPostScriptName(font) as String,
                                         "size": CTFontGetSize(font)]
            if let version = CTFontCopyName(font, kCTFontVersionNameKey) {
                result["version"] = version as String
            }
            return result
        }
        lines.append([
            "start_utf16": start, "end_utf16": start + count,
            "text": (text.string as NSString).substring(with: NSRange(location: start, length: count)),
            "advance": advance, "trailing_whitespace": CTLineGetTrailingWhitespaceWidth(line),
            "ascent": ascent, "descent": descent, "leading": leading,
            "runs": faces,
        ])
        start += count
    } while start < text.length
    results.append(["id": item.id, "width": item.width,
                    "requested_runs": try JSONSerialization.jsonObject(with: JSONEncoder().encode(item.runs)),
                    "requested_faces_resolved": resolvedFonts, "lines": lines])
}
let output: [String: Any] = [
    "engine": "Core Text typesetter", "os": ProcessInfo.processInfo.operatingSystemVersionString,
    "units": "points", "cases": results,
]
FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: output, options: [.prettyPrinted, .sortedKeys]))
