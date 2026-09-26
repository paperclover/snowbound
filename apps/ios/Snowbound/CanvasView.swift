import UIKit

private final class MetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
}

/// An offset into the active outline's text in UTF-16 units, paragraphs joined by a newline.
private final class Position: UITextPosition {
    let value: Int
    init(_ value: Int) { self.value = value }
}

private final class Range: UITextRange {
    let lo: Int
    let hi: Int
    init(_ a: Int, _ b: Int) { (lo, hi) = (min(a, b), max(a, b)) }
    override var start: UITextPosition { Position(lo) }
    override var end: UITextPosition { Position(hi) }
    override var isEmpty: Bool { lo == hi }
}

private final class SelectionRect: UITextSelectionRect {
    private let frame: CGRect
    private let first: Bool
    private let last: Bool
    init(_ frame: CGRect, first: Bool, last: Bool) { (self.frame, self.first, self.last) = (frame, first, last) }
    override var rect: CGRect { frame }
    override var writingDirection: NSWritingDirection { .leftToRight }
    override var containsStart: Bool { first }
    override var containsEnd: Bool { last }
    override var isVertical: Bool { false }
}

private extension UITextPosition { var offset: Int { (self as! Position).value } }
private extension UITextRange {
    var lower: Int { (self as! Range).lo }
    var upper: Int { (self as! Range).hi }
}

/// A canvas page: a scroll view whose pan and pinch drive the canvas viewport, with the page
/// drawn into a Metal layer pinned to the visible bounds, and the active outline's text
/// behind `UITextInput` so the system keyboard, marked text, autocorrection, dictation,
/// selection handles and loupe work on it.
final class CanvasView: UIScrollView, UIScrollViewDelegate, UITextInput, UITextInteractionDelegate,
    UITextSelectionDisplayInteractionDelegate
{
    private let section: Section
    private let page: Int
    private let metal = MetalView()
    /// Sized to the page content at 100%, so the scroll view's zoom scale is the page zoom.
    private let content = UIView()
    private var handle: OpaquePointer?
    private var link: CADisplayLink?
    private var dirty = true
    /// The content's corner at 100%, from which the scroll offset is measured.
    private var origin = CGPoint.zero
    private var syncing = false
    private var scrolled = false
    private lazy var tap = UITapGestureRecognizer(target: self, action: #selector(tapped))
    private let interaction = UITextInteraction(for: .editable)
    /// Draws the system caret, selection highlight and handles; the canvas paints neither.
    private lazy var display = UITextSelectionDisplayInteraction(textInput: self, delegate: self)

    weak var inputDelegate: UITextInputDelegate?
    lazy var tokenizer: UITextInputTokenizer = UITextInputStringTokenizer(textInput: self)

    init(section: Section, page: Int) {
        self.section = section
        self.page = page
        super.init(frame: .zero)
        backgroundColor = .white
        delegate = self
        minimumZoomScale = 0.25
        maximumZoomScale = 4
        keyboardDismissMode = .interactive
        addSubview(content)
        insertSubview(metal, at: 0)
        metal.isUserInteractionEnabled = false
        let layer = metal.layer as! CAMetalLayer
        layer.isOpaque = true
        // Frames reach the screen with the transaction that moves UIKit's caret and handles.
        layer.presentsWithTransaction = true
        addGestureRecognizer(tap)
        interaction.textInput = self
        interaction.delegate = self
        addInteraction(interaction)
        addInteraction(display)
        NotificationCenter.default.addObserver(
            self, selector: #selector(keyboardChanged), name: UIResponder.keyboardWillChangeFrameNotification,
            object: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit {
        link?.invalidate()
        if let handle { sb_view_free(handle) }
    }

    // MARK: Drawing

    override func layoutSubviews() {
        super.layoutSubviews()
        metal.frame = bounds
        let scale = window?.screen.scale ?? 3
        metal.layer.contentsScale = scale
        guard bounds.width > 0, bounds.height > 0 else { return }
        let pixels = CGSize(width: (bounds.width * scale).rounded(), height: (bounds.height * scale).rounded())
        if let handle {
            // Scrolling lays the view out too; only a new size reconfigures the surface.
            guard (metal.layer as! CAMetalLayer).drawableSize != pixels else { return }
            sb_view_resize(handle, Float(bounds.width), Float(bounds.height), Float(scale))
            transform()
        } else if let sectionHandle = section.handle {
            handle = sb_view_new(
                Unmanaged.passUnretained(metal.layer).toOpaque(), sectionHandle, page,
                Float(bounds.width), Float(bounds.height), Float(scale))
            guard let handle else { return }
            sync()
            let fit = (bounds.width - safeAreaInsets.left - safeAreaInsets.right) / content.bounds.width
            zoomScale = min(1, max(0.5, fit))
            home()
            #if DEBUG
            runScript()
            #endif
        }
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        link?.invalidate()
        link = nil
        guard window != nil else { return }
        let link = CADisplayLink(target: self, selector: #selector(frame(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    @objc private func frame(_ link: CADisplayLink) {
        guard let handle else { return }
        if sb_view_frame_pending(handle) { dirty = true }
        guard dirty else { return }
        dirty = false
        sb_view_render(handle)
    }

    // MARK: Scrolling and zoom

    /// Sizes the scroll content to the page's after an edit, keeping the page point at the
    /// view's corner. The scroll view owns the offset: the canvas's own clamping and caret
    /// reveal know neither the navigation bar nor the keyboard, so `transform` overrides them.
    private func sync() {
        guard let handle else { return }
        var bounds: [Float] = [0, 0, 0, 0]
        sb_view_content(handle, &bounds)
        let corner = CGPoint(x: origin.x + contentOffset.x / zoomScale, y: origin.y + contentOffset.y / zoomScale)
        origin = CGPoint(x: CGFloat(bounds[0]), y: CGFloat(bounds[1]))
        let size = CGSize(width: CGFloat(bounds[2]) - origin.x, height: CGFloat(bounds[3]) - origin.y)
        syncing = true
        content.bounds = CGRect(origin: .zero, size: size)
        content.center = CGPoint(x: size.width * zoomScale / 2, y: size.height * zoomScale / 2)
        contentSize = CGSize(width: size.width * zoomScale, height: size.height * zoomScale)
        contentOffset = CGPoint(x: (corner.x - origin.x) * zoomScale, y: (corner.y - origin.y) * zoomScale)
        syncing = false
        transform()
    }

    private func transform() {
        guard let handle, !syncing else { return }
        sb_view_set_transform(
            handle, Float(zoomScale), Float(origin.x + contentOffset.x / zoomScale),
            Float(origin.y + contentOffset.y / zoomScale))
        metal.frame = bounds
        dirty = true
        display.setNeedsSelectionUpdate()
    }

    /// Until the reader scrolls, the page's corner stays below the navigation bar as its
    /// insets settle.
    override func adjustedContentInsetDidChange() {
        super.adjustedContentInsetDidChange()
        home()
    }

    private func home() {
        if !scrolled { contentOffset = CGPoint(x: -adjustedContentInset.left, y: -adjustedContentInset.top) }
    }

    func scrollViewWillBeginDragging(_ scrollView: UIScrollView) { scrolled = true }
    func viewForZooming(in scrollView: UIScrollView) -> UIView? { content }
    func scrollViewDidScroll(_ scrollView: UIScrollView) { transform() }
    func scrollViewDidZoom(_ scrollView: UIScrollView) { transform() }

    @objc private func keyboardChanged(_ notification: Notification) {
        guard let frame = notification.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect,
            let window
        else { return }
        let keyboard = convert(frame, from: window.screen.coordinateSpace)
        let overlap = max(0, bounds.maxY - keyboard.minY - safeAreaInsets.bottom)
        contentInset.bottom = overlap
        verticalScrollIndicatorInsets.bottom = overlap
        revealCaret()
    }

    // MARK: Touch

    /// The canvas places the caret, focuses outlines and starts new ones; the system's text
    /// interaction takes over inside the outline already taking input.
    private func inActiveText(_ point: CGPoint) -> Bool {
        guard let handle else { return false }
        let local = visible(point)
        return isFirstResponder && sb_view_in_active_text(handle, Float(local.x), Float(local.y))
    }

    override func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
        recognizer === tap
            ? !inActiveText(recognizer.location(in: self)) : super.gestureRecognizerShouldBegin(recognizer)
    }

    func interactionShouldBegin(_ interaction: UITextInteraction, at point: CGPoint) -> Bool {
        inActiveText(point)
    }

    @objc private func tapped(_ recognizer: UITapGestureRecognizer) {
        tap(at: recognizer.location(in: self))
    }

    /// A tap at `point` in the scroll view's bounds.
    func tap(at point: CGPoint) {
        guard let handle else { return }
        let point = visible(point)
        edit(external: true) { sb_view_tap(handle, Float(point.x), Float(point.y)) }
        if !isFirstResponder { _ = becomeFirstResponder() }
    }

    /// Runs a change to the page, telling the system when it did not ask for it.
    private func edit(external: Bool = false, _ change: () -> Bool) {
        if external {
            inputDelegate?.selectionWillChange(self)
            inputDelegate?.textWillChange(self)
        }
        let changed = change()
        if external {
            inputDelegate?.textDidChange(self)
            inputDelegate?.selectionDidChange(self)
        }
        if changed {
            sync()
            revealCaret()
        }
    }

    private func revealCaret() {
        guard let handle, isFirstResponder else { return }
        var range: [UInt32] = [0, 0]
        sb_selection(handle, &range)
        let caret = caretRect(for: Position(Int(range[1])))
        scrollRectToVisible(caret.insetBy(dx: -8, dy: -16), animated: false)
    }

    override var canBecomeFirstResponder: Bool { handle != nil }

    override func becomeFirstResponder() -> Bool {
        guard super.becomeFirstResponder() else { return false }
        display.isActivated = true
        return true
    }

    override func resignFirstResponder() -> Bool {
        guard super.resignFirstResponder() else { return false }
        display.isActivated = false
        return true
    }

    /// Pasted text takes the keyboard's language, as Windows gives the clipboard.
    override func paste(_ sender: Any?) {
        guard let handle, let text = UIPasteboard.general.string else { return }
        edit(external: true) { sb_paste(handle, text, textInputMode?.primaryLanguage ?? "") }
    }

    // MARK: Coordinates

    /// A point in the scroll view's bounds as the canvas view's points from its corner.
    private func visible(_ point: CGPoint) -> CGPoint {
        CGPoint(x: point.x - contentOffset.x, y: point.y - contentOffset.y)
    }

    private func bounded(_ rect: [Float]) -> CGRect {
        CGRect(
            x: CGFloat(rect[0]) + contentOffset.x, y: CGFloat(rect[1]) + contentOffset.y,
            width: CGFloat(rect[2]), height: CGFloat(rect[3]))
    }

    // MARK: UIKeyInput

    var hasText: Bool { handle.map { sb_text_length($0) > 0 } ?? false }

    func insertText(_ text: String) {
        guard let handle else { return }
        edit { sb_insert(handle, text) }
    }

    func deleteBackward() {
        guard let handle else { return }
        edit { sb_delete_backward(handle) }
    }

    // MARK: UITextInput

    func text(in range: UITextRange) -> String? {
        guard let handle, let text = sb_text(handle, UInt32(range.lower), UInt32(range.upper)) else { return nil }
        defer { sb_string_free(text) }
        return String(cString: text)
    }

    func replace(_ range: UITextRange, withText text: String) {
        guard let handle else { return }
        edit { sb_replace(handle, UInt32(range.lower), UInt32(range.upper), text) }
    }

    var selectedTextRange: UITextRange? {
        get {
            guard let handle else { return nil }
            var range: [UInt32] = [0, 0]
            sb_selection(handle, &range)
            return Range(Int(range[0]), Int(range[1]))
        }
        set {
            guard let handle, let newValue else { return }
            _ = sb_select(handle, UInt32(newValue.lower), UInt32(newValue.upper))
            dirty = true
            display.setNeedsSelectionUpdate()
        }
    }

    var markedTextRange: UITextRange? {
        guard let handle else { return nil }
        var range: [UInt32] = [0, 0]
        return sb_marked(handle, &range) ? Range(Int(range[0]), Int(range[1])) : nil
    }

    var markedTextStyle: [NSAttributedString.Key: Any]?

    func setMarkedText(_ markedText: String?, selectedRange: NSRange) {
        guard let handle else { return }
        edit {
            sb_set_marked(
                handle, markedText ?? "", UInt32(selectedRange.location),
                UInt32(selectedRange.location + selectedRange.length))
        }
    }

    func unmarkText() {
        guard let handle else { return }
        sb_unmark(handle)
        dirty = true
    }

    var beginningOfDocument: UITextPosition { Position(0) }
    var endOfDocument: UITextPosition { Position(handle.map { Int(sb_text_length($0)) } ?? 0) }

    func textRange(from fromPosition: UITextPosition, to toPosition: UITextPosition) -> UITextRange? {
        Range(fromPosition.offset, toPosition.offset)
    }

    func position(from position: UITextPosition, offset: Int) -> UITextPosition? {
        let target = position.offset + offset
        guard target >= 0, target <= endOfDocument.offset else { return nil }
        return Position(target)
    }

    func position(
        from position: UITextPosition, in direction: UITextLayoutDirection, offset: Int
    ) -> UITextPosition? {
        switch direction {
        case .left: return self.position(from: position, offset: -offset)
        case .right: return self.position(from: position, offset: offset)
        default:
            let caret = caretRect(for: position)
            let step = (direction == .up ? -1 : 1) * CGFloat(offset) * caret.height
            return closestPosition(to: CGPoint(x: caret.midX, y: caret.midY + step))
        }
    }

    func compare(_ position: UITextPosition, to other: UITextPosition) -> ComparisonResult {
        position.offset < other.offset
            ? .orderedAscending : position.offset > other.offset ? .orderedDescending : .orderedSame
    }

    func offset(from: UITextPosition, to toPosition: UITextPosition) -> Int { toPosition.offset - from.offset }

    func position(within range: UITextRange, farthestIn direction: UITextLayoutDirection) -> UITextPosition? {
        direction == .left || direction == .up ? range.start : range.end
    }

    func characterRange(byExtending position: UITextPosition, in direction: UITextLayoutDirection)
        -> UITextRange?
    {
        direction == .left || direction == .up
            ? Range(0, position.offset) : Range(position.offset, endOfDocument.offset)
    }

    func baseWritingDirection(
        for position: UITextPosition, in direction: UITextStorageDirection
    ) -> NSWritingDirection { .leftToRight }

    func setBaseWritingDirection(_ writingDirection: NSWritingDirection, for range: UITextRange) {}

    func firstRect(for range: UITextRange) -> CGRect {
        selectionRects(for: range).first?.rect ?? caretRect(for: range.start)
    }

    func caretRect(for position: UITextPosition) -> CGRect {
        guard let handle else { return .zero }
        var rect: [Float] = [0, 0, 0, 0]
        guard sb_caret_rect(handle, UInt32(position.offset), &rect) else { return .zero }
        var caret = bounded(rect)
        caret.size.width = 2
        return caret
    }

    func selectionRects(for range: UITextRange) -> [UITextSelectionRect] {
        guard let handle else { return [] }
        var rects = [(Float, Float, Float, Float)](repeating: (0, 0, 0, 0), count: 64)
        let count = min(64, sb_range_rects(handle, UInt32(range.lower), UInt32(range.upper), &rects, 64))
        return (0..<count).map {
            let rect = rects[$0]
            return SelectionRect(
                bounded([rect.0, rect.1, rect.2, rect.3]), first: $0 == 0, last: $0 == count - 1)
        }
    }

    func closestPosition(to point: CGPoint) -> UITextPosition? {
        guard let handle else { return nil }
        let local = visible(point)
        return Position(Int(sb_closest(handle, Float(local.x), Float(local.y))))
    }

    func closestPosition(to point: CGPoint, within range: UITextRange) -> UITextPosition? {
        let offset = closestPosition(to: point)?.offset ?? range.lower
        return Position(min(max(offset, range.lower), range.upper))
    }

    func characterRange(at point: CGPoint) -> UITextRange? {
        guard let position = closestPosition(to: point) else { return nil }
        return Range(position.offset, min(position.offset + 1, endOfDocument.offset))
    }
}
