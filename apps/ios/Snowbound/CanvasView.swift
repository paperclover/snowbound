import UIKit

private final class MetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
}

/// An offset into the active outline's shown text in UTF-16 units, paragraphs joined by a
/// newline; collapsed paragraphs are not part of it.
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

/// The page date's fields, from `sb_view_date_request`.
enum DateField: Int8 {
    case date, time
}

/// What a touch lands on; see `sb_view_target`.
private enum Target: UInt8 {
    case page, activeText, text, grip
}

/// Routes the system's undo (shake, three-finger swipe, the keyboard's undo key) to the
/// canvas's history.
private final class CanvasUndoManager: UndoManager {
    weak var canvas: CanvasView?
    override var canUndo: Bool { canvas?.canUndo(redo: false) ?? false }
    override var canRedo: Bool { canvas?.canUndo(redo: true) ?? false }
    override func undo() { canvas?.undo(redo: false) }
    override func redo() { canvas?.undo(redo: true) }
}

/// A canvas page: a scroll view whose pan and pinch drive the canvas viewport, with the page
/// drawn into a Metal layer pinned to the visible bounds, and the active outline's text
/// behind `UITextInput` so the system keyboard, marked text, autocorrection, dictation, text
/// interaction, loupe and edit menu work on it.
final class CanvasView: UIScrollView, UIScrollViewDelegate, UITextInput, UITextInteractionDelegate,
    UITextSelectionDisplayInteractionDelegate, UIGestureRecognizerDelegate
{
    /// The zoom a page opens at once the reader has pinched one.
    private static let zoomKey = "zoom"

    private let section: Section
    private let page: String
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
    /// How far right of the content's corner a page opens.
    private var left: CGFloat = 0
    /// The zoom a double tap returns to.
    private var resting: CGFloat = 1
    private var tapZooming = false
    private lazy var tap = UITapGestureRecognizer(target: self, action: #selector(tapped))
    private lazy var doubleTap: UITapGestureRecognizer = {
        let recognizer = UITapGestureRecognizer(target: self, action: #selector(doubleTapped))
        recognizer.numberOfTapsRequired = 2
        return recognizer
    }()
    /// Moves or widens the focused outline from its grips.
    private lazy var grip = UIPanGestureRecognizer(target: self, action: #selector(gripped))
    /// Where the touch the grip and handle drags look at came down.
    private var touchStart = CGPoint.zero
    /// Drags a selection handle, which the text interaction leaves to the view drawing it.
    private lazy var handleDrag = UIPanGestureRecognizer(target: self, action: #selector(handleDragged))
    /// The selection end a handle drag keeps, and the loupe following the other.
    private var handleAnchor = 0
    private var loupe: UITextLoupeSession?
    private lazy var editMenu = UIEditMenuInteraction(delegate: nil)
    private let interaction = UITextInteraction(for: .editable)
    /// Draws the caret, selection highlight and handles, which the text interaction's
    /// gestures move; the canvas paints neither.
    private lazy var display = UITextSelectionDisplayInteraction(textInput: self, delegate: self)
    private let history = CanvasUndoManager()
    private let spaceHint = Hint()

    weak var inputDelegate: UITextInputDelegate?
    lazy var tokenizer: UITextInputTokenizer = LineTokenizer(canvas: self)
    /// Called when editing starts or stops and after each change, for the bar buttons.
    var onChange: (() -> Void)?
    /// Called once the page is first on screen.
    var onOpened: (() -> Void)?
    /// Called when a tap on the page date asks to change it, with the date it shows.
    var onDate: ((DateField, Date) -> Void)?
    /// Formatting above the keyboard.
    let formatBar = FormatBar()
    /// Room above the page for a bar over it.
    var topInset: CGFloat = 0 {
        didSet {
            contentInset.top = topInset
            home()
        }
    }

    init(section: Section, page: String) {
        self.section = section
        self.page = page
        super.init(frame: .zero)
        backgroundColor = .systemBackground
        delegate = self
        minimumZoomScale = 0.25
        maximumZoomScale = 4
        keyboardDismissMode = .interactive
        // A phone on its side keeps the page clear of the sensor housing, as it keeps the top.
        contentInsetAdjustmentBehavior = .always
        addSubview(content)
        insertSubview(metal, at: 0)
        // Touches land on the scroll view itself, where the text interaction looks for them.
        metal.isUserInteractionEnabled = false
        content.isUserInteractionEnabled = false
        let layer = metal.layer as! CAMetalLayer
        layer.isOpaque = true
        // Frames reach the screen with the transaction that moves UIKit's caret and handles.
        layer.presentsWithTransaction = true
        for recognizer in [tap, doubleTap, grip, handleDrag] {
            recognizer.delegate = self
            addGestureRecognizer(recognizer)
        }
        tap.require(toFail: doubleTap)
        panGestureRecognizer.require(toFail: grip)
        panGestureRecognizer.require(toFail: handleDrag)
        interaction.textInput = self
        interaction.delegate = self
        addInteraction(interaction)
        addInteraction(display)
        display.isActivated = false
        addInteraction(editMenu)
        history.canvas = self
        NotificationCenter.default.addObserver(
            self, selector: #selector(keyboardChanged), name: UIResponder.keyboardWillChangeFrameNotification,
            object: nil)
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (view: CanvasView, _) in view.paper() }
        formatBar.onApply = { [weak self] command in self?.apply(command) }
        formatBar.onDismiss = { [weak self] in _ = self?.resignFirstResponder() }
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
        // Selection views go in at the back, where the opaque page would hide them.
        sendSubviewToBack(metal)
        let scale = window?.screen.scale ?? 3
        metal.layer.contentsScale = scale
        guard bounds.width > 0, bounds.height > 0 else { return }
        let pixels = CGSize(width: (bounds.width * scale).rounded(), height: (bounds.height * scale).rounded())
        if let handle {
            // Scrolling lays the view out too; only a new size reconfigures the surface.
            guard (metal.layer as! CAMetalLayer).drawableSize != pixels else { return }
            sb_view_resize(handle, Float(bounds.width), Float(bounds.height), Float(scale))
            transform()
        } else {
            handle = sb_view_new(
                Unmanaged.passUnretained(metal.layer).toOpaque(), section.handle, page,
                Float(bounds.width), Float(bounds.height), Float(scale))
            guard let handle else { return }
            sb_view_focus(handle, false)
            paper()
            sync()
            let skip = margin
            resting = openingZoom()
            zoomScale = resting
            left = skip * resting
            home()
            onOpened?()
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
        // A frame presented with a transaction shows only once one commits, which an edit
        // that moves no UIKit view would not otherwise cause.
        CATransaction.begin()
        sb_view_render(handle)
        CATransaction.commit()
    }

    /// The page follows the system's appearance, on the desktop's dark paper in dark mode.
    private func paper() {
        guard let handle else { return }
        sb_view_set_dark(handle, traitCollection.userInterfaceStyle == .dark)
        dirty = true
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
    }

    /// The zoom the reader last pinched to, within reason; before that, 100%, where OneNote's
    /// 11 pt body text shows at 17 points.
    private func openingZoom() -> CGFloat {
        guard let zoom = UserDefaults.standard.object(forKey: Self.zoomKey) as? Double else { return 1 }
        // A page opens readable however far the last one was pinched.
        return min(2, max(0.75, zoom))
    }

    /// The empty page left of its first outline, which a phone skips on opening, keeping
    /// room for the tags OneNote hangs left of the text.
    private var margin: CGFloat {
        guard let handle else { return 0 }
        var block: [Float] = [0, 0, 0, 0]
        guard sb_view_block(handle, 0, 0, &block) else { return 0 }
        return max(0, CGFloat(block[0]) - origin.x - 34)
    }

    /// Until the reader scrolls, the page's corner stays below the navigation bar as its
    /// insets settle.
    override func adjustedContentInsetDidChange() {
        super.adjustedContentInsetDidChange()
        home()
    }

    private func home() {
        if !scrolled { contentOffset = CGPoint(x: left - adjustedContentInset.left, y: -adjustedContentInset.top) }
    }

    func scrollViewWillBeginDragging(_ scrollView: UIScrollView) { scrolled = true }
    func scrollViewWillBeginZooming(_ scrollView: UIScrollView, with view: UIView?) { scrolled = true }
    func viewForZooming(in scrollView: UIScrollView) -> UIView? { content }
    func scrollViewDidScroll(_ scrollView: UIScrollView) { transform() }
    func scrollViewDidZoom(_ scrollView: UIScrollView) {
        transform()
        // Scrolling carries the caret and handles along; zooming moves them within the page.
        display.setNeedsSelectionUpdate()
    }

    func scrollViewDidEndZooming(_ scrollView: UIScrollView, with view: UIView?, atScale scale: CGFloat) {
        if tapZooming {
            tapZooming = false
        } else {
            resting = scale
            UserDefaults.standard.set(Double(scale), forKey: Self.zoomKey)
        }
    }

    @objc private func doubleTapped(_ recognizer: UITapGestureRecognizer) {
        zoom(at: recognizer.location(in: self))
    }

    /// Zooms to fit the outline nearest `point` across the view, or back out, as a double
    /// tap does in Safari.
    func zoom(at point: CGPoint) {
        guard let handle else { return }
        tapZooming = true
        var block: [Float] = [0, 0, 0, 0]
        let local = visible(point)
        guard zoomScale <= resting * 1.05, sb_view_block(handle, Float(local.x), Float(local.y), &block) else {
            setZoomScale(resting, animated: true)
            return
        }
        let width = bounds.inset(by: safeAreaInsets).width
        let margin: CGFloat = 12
        let scale = min(maximumZoomScale, (width - 2 * margin) / (CGFloat(block[2] - block[0])))
        guard scale > zoomScale * 1.05 else {
            setZoomScale(resting, animated: true)
            return
        }
        let x = CGFloat(block[0]) - origin.x - margin / scale
        let y = point.y / zoomScale - bounds.height / scale / 2
        zoom(to: CGRect(x: x, y: y, width: width / scale, height: bounds.height / scale), animated: true)
    }

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

    private func target(_ point: CGPoint) -> Target {
        guard let handle else { return .page }
        let local = visible(point)
        return Target(rawValue: sb_view_target(handle, Float(local.x), Float(local.y))) ?? .page
    }

    func gestureRecognizer(_ recognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
        if recognizer === grip || recognizer === handleDrag { touchStart = touch.location(in: self) }
        return true
    }

    override func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
        switch recognizer {
        case tap:
            !readOnly && [.page, .grip].contains(target(recognizer.location(in: self)))
                && !onHandle(recognizer.location(in: self))
        case doubleTap: target(recognizer.location(in: self)) == .page
        case grip: !readOnly && target(touchStart) == .grip
        case handleDrag: onHandle(touchStart)
        default: super.gestureRecognizerShouldBegin(recognizer)
        }
    }

    /// The system's text interaction works in the outline taking input; a touch on another
    /// outline's text focuses it first, as a tap there would.
    func interactionShouldBegin(_ interaction: UITextInteraction, at point: CGPoint) -> Bool {
        if readOnly { return false }
        if onHandle(point) { return true }
        switch target(point) {
        case .activeText: break
        case .text: press(at: point)
        case .page, .grip: return false
        }
        if !isFirstResponder { _ = becomeFirstResponder() }
        return true
    }

    /// Whether `point` is on a selection handle, whose knob hangs off the text.
    private func onHandle(_ point: CGPoint) -> Bool {
        guard isFirstResponder, let range = selectedTextRange, !range.isEmpty else { return false }
        return [range.start, range.end].contains {
            caretRect(for: $0).insetBy(dx: -22, dy: -22).contains(point)
        }
    }

    @objc private func tapped(_ recognizer: UITapGestureRecognizer) {
        tap(at: recognizer.location(in: self))
    }

    /// A tap at `point` in the scroll view's bounds: places the caret, focuses an outline or
    /// starts a new one.
    func tap(at point: CGPoint) {
        press(at: point)
        var seconds: Int64 = 0
        if let handle, let field = DateField(rawValue: sb_view_date_request(handle, &seconds)) {
            onDate?(field, Date(timeIntervalSince1970: TimeInterval(seconds)))
        } else if !isFirstResponder {
            _ = becomeFirstResponder()
        }
    }

    /// Gives the page `date`, which the date and time under its title show.
    func changeDate(_ date: Date) {
        guard let handle else { return }
        let (day, time) = titleDate(date)
        edit(external: true) { sb_view_change_date(handle, Int64(date.timeIntervalSince1970.rounded()), day, time) }
    }

    private func press(at point: CGPoint) {
        guard let handle else { return }
        let point = visible(point)
        edit(external: true) {
            let pressed = sb_view_press(handle, Float(point.x), Float(point.y))
            return sb_view_release(handle) || pressed
        }
    }

    @objc private func handleDragged(_ recognizer: UIPanGestureRecognizer) {
        guard let range = selectedTextRange else { return }
        // The finger sits below the knob; aim at the text line above it.
        let point = recognizer.location(in: self)
        let aim = CGPoint(x: point.x, y: point.y - 16)
        switch recognizer.state {
        case .began:
            let start = caretRect(for: range.start)
            let end = caretRect(for: range.end)
            let nearStart = hypot(start.midX - touchStart.x, start.midY - touchStart.y)
                < hypot(end.midX - touchStart.x, end.midY - touchStart.y)
            handleAnchor = nearStart ? range.upper : range.lower
            let handle = display.handleViews.min {
                hypot($0.center.x - touchStart.x, $0.center.y - touchStart.y)
                    < hypot($1.center.x - touchStart.x, $1.center.y - touchStart.y)
            }
            loupe = UITextLoupeSession.begin(at: aim, fromSelectionWidgetView: handle, in: self)
            editMenu.dismissMenu()
            fallthrough
        case .changed:
            guard let position = closestPosition(to: aim) else { return }
            // A handle stops one character short of the other.
            let step = position.offset < handleAnchor || (position.offset == handleAnchor && handleAnchor > 0) ? -1 : 1
            let end = position.offset == handleAnchor ? handleAnchor + step : position.offset
            let moved = Range(handleAnchor, min(max(end, 0), endOfDocument.offset))
            edit(external: true) { choose(moved) }
            let caret = caretRect(for: position)
            loupe?.move(to: CGPoint(x: caret.midX, y: caret.midY), withCaretRect: caret, trackingCaret: false)
        default:
            loupe?.invalidate()
            loupe = nil
            let end = caretRect(for: range.end)
            editMenu.presentEditMenu(with: UIEditMenuConfiguration(
                identifier: nil, sourcePoint: CGPoint(x: end.midX, y: caretRect(for: range.start).minY)))
        }
    }

    @objc private func gripped(_ recognizer: UIPanGestureRecognizer) {
        guard let handle else { return }
        let point = visible(recognizer.location(in: self))
        switch recognizer.state {
        case .began:
            let start = visible(touchStart)
            _ = sb_view_press(handle, Float(start.x), Float(start.y))
            _ = sb_view_drag(handle, Float(point.x), Float(point.y))
            // The caret and handles stay behind until the outline lands.
            display.isActivated = false
            editMenu.dismissMenu()
            dirty = true
        case .changed:
            _ = sb_view_drag(handle, Float(point.x), Float(point.y))
            dirty = true
        default:
            display.isActivated = isFirstResponder
            edit(external: true) { sb_view_release(handle) }
            spaceHint.hide()
        }
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
        dirty = true
        if changed {
            sync()
            revealCaret()
        }
        display.setNeedsSelectionUpdate()
        changedSelection()
    }

    /// Tells the bar buttons and the format bar what the selection now has.
    private func changedSelection() {
        formatBar.show(format)
        onChange?()
    }

    private func revealCaret() {
        guard handle != nil, isFirstResponder, let range = selectedTextRange else { return }
        reveal(caretRect(for: range.end))
    }

    private func reveal(_ rect: CGRect) {
        scrollRectToVisible(rect.insetBy(dx: -8, dy: -16), animated: false)
    }

    // MARK: Responder

    override var canBecomeFirstResponder: Bool { handle != nil && !readOnly }

    override var inputAccessoryView: UIView? { formatBar }

    override func becomeFirstResponder() -> Bool {
        guard super.becomeFirstResponder() else { return false }
        if let handle { sb_view_focus(handle, true) }
        display.isActivated = true
        dirty = true
        changedSelection()
        return true
    }

    override func resignFirstResponder() -> Bool {
        guard super.resignFirstResponder() else { return false }
        if let handle { sb_view_focus(handle, false) }
        display.isActivated = false
        dirty = true
        onChange?()
        return true
    }

    override var undoManager: UndoManager? { history }

    func canUndo(redo: Bool) -> Bool { handle.map { sb_view_can_undo($0, redo) } ?? false }

    func undo(redo: Bool) {
        guard let handle else { return }
        edit(external: true) { sb_view_undo(handle, redo) }
    }

    private var selection: (lo: Int, hi: Int) {
        guard let range = selectedTextRange else { return (0, 0) }
        return (range.lower, range.upper)
    }

    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        let (lo, hi) = selection
        switch action {
        case #selector(copy(_:)), #selector(cut(_:)), #selector(delete(_:)),
            #selector(toggleBoldface(_:)), #selector(toggleItalics(_:)), #selector(toggleUnderline(_:)):
            return lo < hi
        case #selector(paste(_:)): return UIPasteboard.general.hasStrings
        case #selector(select(_:)): return lo == hi && hasText
        case #selector(selectAll(_:)): return hasText && (lo > 0 || hi < endOfDocument.offset)
        default: return super.canPerformAction(action, withSender: sender)
        }
    }

    /// A page selection copies every outline, as OneNote 2010 copies it.
    override func copy(_ sender: Any?) {
        guard let handle, let text = take(sb_view_copy(handle, false)) else { return }
        UIPasteboard.general.string = text
    }

    override func cut(_ sender: Any?) {
        guard let handle else { return }
        var text: String?
        edit(external: true) {
            text = take(sb_view_copy(handle, true))
            return text != nil
        }
        if let text { UIPasteboard.general.string = text }
    }

    override func delete(_ sender: Any?) {
        guard let range = selectedTextRange else { return }
        edit(external: true) { replacing(range, with: "") }
    }

    /// Pasted text takes the keyboard's language, as Windows gives the clipboard.
    override func paste(_ sender: Any?) {
        guard let handle, let text = UIPasteboard.general.string else { return }
        edit(external: true) { sb_paste(handle, text, textInputMode?.primaryLanguage ?? "") }
    }

    override func select(_ sender: Any?) {
        guard let caret = selectedTextRange?.start,
            let word = tokenizer.rangeEnclosingPosition(caret, with: .word, inDirection: .storage(.backward))
                ?? tokenizer.rangeEnclosingPosition(caret, with: .word, inDirection: .storage(.forward))
        else { return }
        edit(external: true) { choose(word) }
    }

    override func selectAll(_ sender: Any?) {
        guard let handle else { return }
        edit(external: true) { sb_select_more(handle) }
    }

    override func toggleBoldface(_ sender: Any?) { apply(0) }
    override func toggleItalics(_ sender: Any?) { apply(1) }
    override func toggleUnderline(_ sender: Any?) { apply(2) }

    /// Tags and the highlighter join the system's text actions.
    func editMenu(for textRange: UITextRange, suggestedActions: [UIMenuElement]) -> UIMenu? {
        guard let handle else { return nil }
        let bits = sb_view_format(handle)
        let tag = UIMenu(
            title: "Tag", image: UIImage(systemName: "tag"),
            children: Tags.menu(bits: bits) { [weak self] in self?.apply($0) })
        var extra: [UIMenuElement] = [tag]
        if !textRange.isEmpty {
            extra.insert(
                UIAction(title: "Highlight", image: UIImage(systemName: "highlighter")) { [weak self] _ in
                    self?.apply(10)
                }, at: 0)
        }
        return UIMenu(children: suggestedActions + [UIMenu(options: .displayInline, children: extra)])
    }

    // MARK: Page

    /// A conflict page, which shows what is stored and takes no edits.
    var readOnly: Bool { handle.map(sb_view_read_only) ?? false }

    /// The title as typed; nil on a page without an editable title.
    var pageTitle: String? { handle.flatMap { take(sb_view_title($0)) } }

    var pageText: String { handle.flatMap { take(sb_view_page_text($0)) } ?? "" }

    /// The selection's formatting, from `sb_view_format`.
    var format: UInt64 { handle.map(sb_view_format) ?? 0 }

    var pagePaper: Paper? { handle.flatMap { decode(Paper.self, sb_view_paper($0)) } }

    /// Gives the page colour `color` and rule lines `ruled` of `paper.rules`, or none.
    func setPaper(color: [UInt8]?, ruled: Int?) {
        guard let handle else { return }
        edit(external: true) {
            sb_view_set_paper(
                handle, color.map { Int16($0[0]) } ?? -1, color?[1] ?? 0, color?[2] ?? 0, Int8(ruled ?? -1))
        }
    }

    /// Puts template `name`'s art behind the page, or none.
    func setArt(_ name: String?) {
        guard let handle else { return }
        edit(external: true) { sb_view_set_art(handle, name) }
    }

    /// Insert Space: the next drag moves what lies below or right of where it starts.
    func insertSpace() {
        guard let handle else { return }
        sb_view_insert_space(handle)
        spaceHint.show("Drag down or right to add space", in: self)
    }

    /// Types the date, time or both as the system writes them short, and a space, as
    /// OneNote's Insert Date and Time do.
    func insertDate(_ date: Bool, time: Bool) {
        let now = Date()
        let text = DateFormatter.localizedString(
            from: now, dateStyle: date ? .short : .none, timeStyle: time ? .short : .none)
        if !isFirstResponder { _ = becomeFirstResponder() }
        insertText(text + " ")
    }

    /// Selects paragraph `id` and brings it into view.
    func selectParagraph(_ id: String) -> Bool {
        guard let handle else { return false }
        var found = false
        edit(external: true) {
            found = sb_view_select_paragraph(handle, id)
            return found
        }
        if found { reveal(selectedTextRange.map { firstRect(for: $0) } ?? .zero) }
        return found
    }

    /// Applies `sb_view_apply` formatting to the selection.
    func apply(_ command: UInt8) {
        guard let handle else { return }
        edit(external: true) { sb_view_apply(handle, command) }
    }

    /// Shows the page as stored; with `discard`, after an edit was refused.
    func reload(discard: Bool) {
        guard let handle else { return }
        edit(external: true) { sb_view_reload(handle, discard) }
    }

    /// Selects the first place `query` occurs and brings it into view.
    func find(_ query: String) -> Bool {
        guard let handle else { return false }
        var found = false
        edit(external: true) {
            found = sb_view_find(handle, query)
            return found
        }
        if found { reveal(selectedTextRange.map { firstRect(for: $0) } ?? .zero) }
        return found
    }

    /// Puts the caret at the end of the page title.
    func focusTitle() -> Bool {
        guard let handle else { return false }
        var focused = false
        edit(external: true) {
            focused = sb_view_focus_title(handle)
            return focused
        }
        return focused
    }

    /// Puts a JPEG or PNG after the caret's paragraph at `size` points.
    func insertPicture(_ data: Data, size: CGSize) {
        guard let handle else { return }
        edit(external: true) {
            data.withUnsafeBytes { bytes in
                sb_view_insert_picture(
                    handle, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count, Float(size.width),
                    Float(size.height))
            }
        }
    }

    /// Commits marked text, so it is stored before the app leaves the screen.
    func commitComposition() {
        guard markedTextRange != nil else { return }
        inputDelegate?.textWillChange(self)
        unmarkText()
        inputDelegate?.textDidChange(self)
    }

    override var keyCommands: [UIKeyCommand]? {
        guard isFirstResponder else { return nil }
        // Ctrl+1 to Ctrl+9 in OneNote, where the Mac's Command stands for Control.
        let tags = Tags.all.prefix(Tags.keyed).enumerated().map { index, tag in
            UIKeyCommand(
                title: tag.name, action: #selector(formatted), input: "\(index + 1)", modifierFlags: .command,
                propertyList: 16 + index)
        }
        // The desktop's chords, as OneNote for Mac's.
        let formats: [(String, String, UIKeyModifierFlags, Int)] = [
            ("Strikethrough", "x", [.command, .shift], 3),
            ("Bullets", ".", .command, 4),
            ("Numbering", "/", .command, 5),
            ("Increase Indent", "]", .command, 6),
            ("Decrease Indent", "[", .command, 7),
            ("Remove Tag", "0", [.command, .control], 8),
            ("Clear Formatting", "n", [.command, .shift], 9),
            ("Highlight", "h", [.command, .control], 10),
            ("Decrease Indent", "\t", .shift, 7),
        ]
        let commands = tags + formats.map { title, input, modifiers, command in
            UIKeyCommand(
                title: title, action: #selector(formatted), input: input, modifierFlags: modifiers,
                propertyList: command)
        }
        for command in commands { command.wantsPriorityOverSystemBehavior = true }
        let done = UIKeyCommand(title: "Done", action: #selector(finished), input: UIKeyCommand.inputEscape)
        return commands + [done]
    }

    @objc private func formatted(_ command: UIKeyCommand) {
        guard let command = command.propertyList as? Int else { return }
        apply(UInt8(command))
    }

    @objc private func finished() { _ = resignFirstResponder() }

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

    private func replacing(_ range: UITextRange, with text: String) -> Bool {
        guard let handle else { return false }
        return sb_replace(handle, UInt32(range.lower), UInt32(range.upper), text)
    }

    func replace(_ range: UITextRange, withText text: String) {
        edit { replacing(range, with: text) }
    }

    private func choose(_ range: UITextRange) -> Bool {
        guard let handle else { return false }
        return sb_select(handle, UInt32(range.lower), UInt32(range.upper))
    }

    var selectedTextRange: UITextRange? {
        get {
            guard let handle else { return nil }
            var range: [UInt32] = [0, 0]
            sb_selection(handle, &range)
            return Range(Int(range[0]), Int(range[1]))
        }
        set {
            guard let newValue else { return }
            _ = choose(newValue)
            display.setNeedsSelectionUpdate()
            dirty = true
            changedSelection()
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
        caret.origin.x -= 1
        caret.size.width = 2
        return caret
    }

    func selectionRects(for range: UITextRange) -> [UITextSelectionRect] {
        guard let handle else { return [] }
        let capacity = 256
        var rects = [(Float, Float, Float, Float)](repeating: (0, 0, 0, 0), count: capacity)
        let count = min(capacity, sb_range_rects(handle, UInt32(range.lower), UInt32(range.upper), &rects, capacity))
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

/// Words and sentences come from the text; lines come from the canvas's layout, which the
/// string tokenizer cannot see, so a tap after a line's last word stays on that line.
private final class LineTokenizer: UITextInputStringTokenizer {
    private unowned let canvas: CanvasView

    init(canvas: CanvasView) {
        self.canvas = canvas
        super.init(textInput: canvas)
    }

    private func forward(_ direction: UITextDirection) -> Bool {
        [UITextStorageDirection.forward.rawValue, UITextLayoutDirection.right.rawValue,
         UITextLayoutDirection.down.rawValue].contains(direction.rawValue)
    }

    private func line(_ offset: Int) -> CGFloat { canvas.caretRect(for: Position(offset)).minY.rounded() }

    private func atEdge(_ offset: Int, forward: Bool) -> Bool {
        let end = canvas.endOfDocument.offset
        return forward
            ? offset >= end || line(offset + 1) != line(offset)
            : offset <= 0 || line(offset - 1) != line(offset)
    }

    private func edge(_ offset: Int, forward: Bool) -> Int {
        var offset = offset
        while !atEdge(offset, forward: forward) { offset += forward ? 1 : -1 }
        return offset
    }

    override func isPosition(
        _ position: UITextPosition, atBoundary granularity: UITextGranularity, inDirection direction: UITextDirection
    ) -> Bool {
        guard granularity == .line else {
            return super.isPosition(position, atBoundary: granularity, inDirection: direction)
        }
        return atEdge(position.offset, forward: forward(direction))
    }

    override func position(
        from position: UITextPosition, toBoundary granularity: UITextGranularity, inDirection direction: UITextDirection
    ) -> UITextPosition? {
        guard granularity == .line else {
            return super.position(from: position, toBoundary: granularity, inDirection: direction)
        }
        return Position(edge(position.offset, forward: forward(direction)))
    }

    override func rangeEnclosingPosition(
        _ position: UITextPosition, with granularity: UITextGranularity, inDirection direction: UITextDirection
    ) -> UITextRange? {
        guard granularity == .line else {
            return super.rangeEnclosingPosition(position, with: granularity, inDirection: direction)
        }
        return Range(edge(position.offset, forward: false), edge(position.offset, forward: true))
    }

    override func isPosition(
        _ position: UITextPosition, withinTextUnit granularity: UITextGranularity, inDirection direction: UITextDirection
    ) -> Bool {
        granularity == .line || super.isPosition(position, withinTextUnit: granularity, inDirection: direction)
    }
}

/// A short instruction floating at the bottom of the page until the gesture it asks for.
private final class Hint: UIVisualEffectView {
    private let label = UILabel()

    init() {
        super.init(effect: UIBlurEffect(style: .systemThickMaterial))
        label.font = .preferredFont(forTextStyle: .subheadline)
        label.textAlignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        contentView.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 16),
            label.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -16),
            label.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 10),
            label.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -10),
        ])
        layer.cornerRadius = 18
        clipsToBounds = true
        isUserInteractionEnabled = false
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Shows `text` over the bottom of `view`'s visible area, and says it.
    func show(_ text: String, in view: UIScrollView) {
        label.text = text
        guard let host = view.superview else { return }
        translatesAutoresizingMaskIntoConstraints = false
        host.addSubview(self)
        NSLayoutConstraint.activate([
            centerXAnchor.constraint(equalTo: host.centerXAnchor),
            bottomAnchor.constraint(equalTo: host.keyboardLayoutGuide.topAnchor, constant: -16),
        ])
        alpha = 0
        UIView.animate(withDuration: 0.2) { self.alpha = 1 }
        UIAccessibility.post(notification: .announcement, argument: text)
    }

    func hide() {
        guard superview != nil else { return }
        UIView.animate(withDuration: 0.2, animations: { self.alpha = 0 }) { _ in self.removeFromSuperview() }
    }
}
