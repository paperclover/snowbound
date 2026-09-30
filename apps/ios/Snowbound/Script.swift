#if DEBUG
import UIKit

private var scriptedPage = false

extension CanvasView {
    /// Replays `SNOWBOUND_SCRIPT` through the calls touch and the keyboard make, to check
    /// input without a finger: steps joined by `|`, such as
    /// `tap:120,300|type:hi|mark:かな|unmark|return|delete|select:2,9|scroll:0,600|zoom:1.5|shot:a`.
    /// `tap` and `doubletap` take points from the view's corner, `select` text offsets,
    /// `scroll` a content offset; `format:N` applies `sb_view_apply` formatting, `find:word`
    /// selects a match, `title` edits the title, `picture` inserts a drawn picture, `selectall`
    /// is the Select All command; `pencil:x,y;x,y;…` draws through points as the Pencil does,
    /// `tool:pen:N`, `tool:shape:N`, `tool:eraser` or `tool:lasso` gives it a tool, `picker` shows or hides the
    /// drawing tools, `pencildoubletap` is the Pencil's double tap, and `deleteink` deletes
    /// what the lasso picked; `rotate:landscape` or `rotate:portrait` turns the device; `done`
    /// ends editing, `tree` saves the view hierarchy to
    /// Documents/tree.txt, and `shot:a` the window to Documents/a.png, as a device has no
    /// screenshot command.
    func runScript() {
        // Only the first page shown runs it, not every page opened afterwards.
        guard !scriptedPage, let script = ProcessInfo.processInfo.environment["SNOWBOUND_SCRIPT"] else { return }
        scriptedPage = true
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
        case "pencil":
            let points = argument.split(separator: ";").map {
                let values = $0.split(separator: ",").compactMap { Double($0) }
                return CGPoint(x: values[0] + contentOffset.x, y: values[1] + contentOffset.y)
            }
            inkPressed(points[0])
            inkMoved(points.dropFirst())
            inkReleased()
        case "tool":
            let parts = argument.split(separator: ":")
            switch parts.first {
            case "pen": setInkTool(.pen(UInt8(parts.last ?? "") ?? 0))
            case "eraser": setInkTool(.eraser)
            case "lasso": setInkTool(.lasso)
            case "shape": setInkTool(.shape(UInt8(parts.last ?? "") ?? 0))
            default: break
            }
        case "picker": showPicker(!pickerShown)
        case "pencildoubletap": pencilInteractionDidTap(UIPencilInteraction())
        case "deleteink": deleteInk()
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
        case "rotate":
            let orientation: UIInterfaceOrientationMask = argument == "portrait" ? .portrait : .landscapeRight
            window?.windowScene?.requestGeometryUpdate(.iOS(interfaceOrientations: orientation))
        case "done": _ = resignFirstResponder()
        case "selectall": selectAll(nil)
        case "format": apply(UInt8(argument) ?? 0)
        case "find": _ = find(argument)
        case "title": _ = focusTitle()
        case "picture":
            let image = UIGraphicsImageRenderer(size: CGSize(width: 320, height: 200)).image { context in
                UIColor.systemTeal.setFill()
                context.fill(CGRect(x: 0, y: 0, width: 320, height: 200))
                UIColor.systemYellow.setFill()
                context.cgContext.fillEllipse(in: CGRect(x: 110, y: 50, width: 100, height: 100))
            }
            if let data = image.jpegData(compressionQuality: 0.9) { insertPicture(data, size: CGSize(width: 240, height: 150)) }
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

/// Logs `presenter changed` whenever a coordinated write reaches `SNOWBOUND_PRESENTER`, a
/// file, to check that saving tells file providers about it.
final class Presenter: NSObject, NSFilePresenter {
    private static var watching: Presenter?
    let presentedItemURL: URL?
    let presentedItemOperationQueue = OperationQueue()

    private init(_ url: URL) { presentedItemURL = url }

    static func watch() {
        guard let path = ProcessInfo.processInfo.environment["SNOWBOUND_PRESENTER"] else { return }
        let presenter = Presenter(URL(fileURLWithPath: path))
        NSFileCoordinator.addFilePresenter(presenter)
        watching = presenter
    }

    func presentedItemDidChange() { NSLog("presenter changed") }
}
#endif
