#if DEBUG
import UIKit

extension CanvasView {
    /// Replays `SNOWBOUND_SCRIPT` through the calls touch and the keyboard make, to check
    /// input without a finger: steps joined by `|`, such as
    /// `tap:120,300|type:hi|mark:かな|unmark|return|delete|select:2,9|scroll:0,600|zoom:1.5|shot:a`.
    /// `tap` and `doubletap` take points from the view's corner, `select` text offsets,
    /// `scroll` a content offset; `done` ends editing, `tree` saves the view hierarchy to
    /// Documents/tree.txt, and `shot:a` the window to Documents/a.png, as a device has no
    /// screenshot command.
    func runScript() {
        guard let script = ProcessInfo.processInfo.environment["SNOWBOUND_SCRIPT"] else { return }
        for (index, step) in script.split(separator: "|").enumerated() {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1 + Double(index) * 0.4) { [weak self] in
                self?.perform(String(step))
            }
        }
    }

    private func perform(_ step: String) {
        let (command, argument) = step.firstIndex(of: ":").map {
            (String(step[..<$0]), String(step[step.index(after: $0)...]))
        } ?? (step, "")
        NSLog("script \(command) \(argument)")
        switch command {
        case "tap":
            let values = argument.split(separator: ",").compactMap { Double($0) }
            tap(at: CGPoint(x: values[0] + contentOffset.x, y: values[1] + contentOffset.y))
        case "type": insertText(argument)
        case "return": insertText("\n")
        case "delete": deleteBackward()
        case "mark": setMarkedText(argument, selectedRange: NSRange(location: argument.utf16.count, length: 0))
        case "unmark": unmarkText()
        case "select":
            let values = argument.split(separator: ",").compactMap { Int($0) }
            // As a gesture would, the change reaches the system's text interaction.
            inputDelegate?.selectionWillChange(self)
            defer { inputDelegate?.selectionDidChange(self) }
            selectedTextRange = textRange(
                from: position(from: beginningOfDocument, offset: values[0])!,
                to: position(from: beginningOfDocument, offset: values[1])!)
        case "scroll":
            let values = argument.split(separator: ",").compactMap { Double($0) }
            setContentOffset(CGPoint(x: values[0], y: values[1]), animated: true)
        case "zoom": setZoomScale(Double(argument) ?? 1, animated: false)
        case "doubletap":
            let values = argument.split(separator: ",").compactMap { Double($0) }
            zoom(at: CGPoint(x: values[0] + contentOffset.x, y: values[1] + contentOffset.y))
        case "done": _ = resignFirstResponder()
        case "tree":
            let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            try? "\(value(forKey: "recursiveDescription") ?? "")".write(
                to: documents.appendingPathComponent("tree.txt"), atomically: true, encoding: .utf8)
        case "shot":
            guard let window else { break }
            let image = UIGraphicsImageRenderer(bounds: window.bounds).image { _ in
                window.drawHierarchy(in: window.bounds, afterScreenUpdates: false)
            }
            let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            try? image.pngData()?.write(to: documents.appendingPathComponent("\(argument).png"))
        default: NSLog("script: unknown step \(step)")
        }
        if let range = selectedTextRange, let all = textRange(from: beginningOfDocument, to: endOfDocument) {
            NSLog("script selection \(offset(from: beginningOfDocument, to: range.start))..\(offset(from: beginningOfDocument, to: range.end)) text \(text(in: all) ?? "nil")")
        }
    }
}
#endif
