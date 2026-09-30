import UIKit

/// What a touch on the page does: `sb_view_set_tool`'s tools, a pen by its place in the
/// section's `Pen.gallery`, a shape by its number.
enum InkTool: Equatable {
    case select
    case pen(UInt8)
    case eraser
    case lasso
    case shape(UInt8)

    var code: (tool: UInt8, detail: UInt8) {
        switch self {
        case .select: (0, 0)
        case .pen(let pen): (1, pen)
        case .eraser: (2, 0)
        case .lasso: (3, 0)
        case .shape(let shape): (4, shape)
        }
    }
}

/// A pen of a section's gallery, from `sb_pens`.
struct Pen {
    /// Nil draws in the paper's ink.
    let color: UIColor?
    /// In HIMETRIC.
    let width: Float
    let highlighter: Bool

    /// The pens under a section of tab colour `section`, a COLORREF: its accent, then
    /// OneNote 2010's favourites, as the desktop's gallery.
    static func gallery(_ section: UInt32) -> [Pen] {
        guard let text = take(sb_pens(section)),
            let rows = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [[Any]]
        else { return [] }
        return rows.compactMap { row in
            guard row.count == 3, let width = row[1] as? NSNumber, let highlighter = row[2] as? Bool else { return nil }
            return Pen(
                color: (row[0] as? [NSNumber]).map { UIColor(rgb: $0.map(\.uint8Value)) }, width: width.floatValue,
                highlighter: highlighter)
        }
    }
}

/// Follows one touch from the moment it lands, gathering every point the hardware sampled,
/// for the canvas's pens, eraser and lasso. A second finger cancels a finger's stroke, so two
/// fingers scroll; a hand resting beside the Pencil is ignored.
final class InkGesture: UIGestureRecognizer {
    /// Points the touch passed since the action last took them, in the view's bounds.
    var points: [CGPoint] = []
    private var tracked: UITouch?

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
        for touch in touches {
            if tracked == nil {
                tracked = touch
                points = [touch.preciseLocation(in: view)]
                state = .began
            } else if tracked?.type == .direct {
                state = .cancelled
            } else {
                ignore(touch, for: event)
            }
        }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let tracked, touches.contains(tracked) else { return }
        points += (event.coalescedTouches(for: tracked) ?? [tracked]).map { $0.preciseLocation(in: view) }
        state = .changed
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
        guard let tracked, touches.contains(tracked) else { return }
        points.append(tracked.preciseLocation(in: view))
        state = .ended
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
        state = .cancelled
    }

    override func reset() {
        tracked = nil
        points = []
    }
}

/// The drawing tools floating over the page, as PencilKit's tool picker lays them out: Select
/// & Type, where a finger scrolls and selects, then pen, highlighter, eraser, lasso and
/// shapes, which a finger draws with too, and the pen's colour and width.
final class InkPicker: UIView {
    /// A tool picked, or Select & Type as nil.
    var onPick: ((InkTool?) -> Void)?
    var onClose: (() -> Void)?

    /// Line, arrow, rectangle and oval, in `sb_view_set_tool`'s order.
    private static let shapes = [
        ("line.diagonal", "Line"), ("arrow.up.right", "Arrow"), ("rectangle", "Rectangle"), ("oval", "Oval"),
    ]
    private let pens: [Pen]
    private let backdrop = UIVisualEffectView(effect: UIBlurEffect(style: .systemChromeMaterial))
    private let row = UIStackView()
    private let select = InkPicker.tool("hand.point.up.left", "Select and Type")
    private let pen = InkPicker.tool("pencil.tip", "Pen")
    private let highlighter = InkPicker.tool("highlighter", "Highlighter")
    private let eraser = InkPicker.tool("eraser", "Eraser")
    private let lasso = InkPicker.tool("lasso", "Lasso")
    private let shape = InkPicker.tool("oval", "Shapes")
    private let color = UIButton(configuration: .plain())
    /// The pen, highlighter and shape picked last, which their buttons return to.
    private var lastPen: UInt8 = 0
    private var lastHighlighter: UInt8
    private var lastShape: UInt8 = 3

    init(pens: [Pen]) {
        self.pens = pens
        lastHighlighter = UInt8(pens.firstIndex(where: \.highlighter) ?? 0)
        super.init(frame: .zero)
        layer.shadowColor = UIColor.black.cgColor
        layer.shadowOpacity = 0.18
        layer.shadowRadius = 12
        layer.shadowOffset = CGSize(width: 0, height: 4)
        backdrop.layer.cornerRadius = 22
        backdrop.layer.cornerCurve = .continuous
        backdrop.clipsToBounds = true
        backdrop.layer.borderWidth = 1 / UIScreen.main.scale
        backdrop.layer.borderColor = UIColor.separator.cgColor
        let picks: [(UIButton, () -> InkTool?)] = [
            (select, { nil }),
            (pen, { [unowned self] in .pen(lastPen) }),
            (highlighter, { [unowned self] in .pen(lastHighlighter) }),
            (eraser, { .eraser }),
            (lasso, { .lasso }),
        ]
        for (button, pick) in picks {
            button.addAction(UIAction { [weak self] _ in self?.onPick?(pick()) }, for: .primaryActionTriggered)
            row.addArrangedSubview(button)
        }
        shape.menu = UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak self] done in done(self?.shapeMenu() ?? []) }
        ])
        shape.showsMenuAsPrimaryAction = true
        row.addArrangedSubview(shape)
        row.setCustomSpacing(8, after: select)
        row.addArrangedSubview(Self.divider())
        color.configuration?.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(pointSize: 24)
        color.widthAnchor.constraint(equalToConstant: 40).isActive = true
        color.menu = UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak self] done in done(self?.colorMenu() ?? []) }
        ])
        color.showsMenuAsPrimaryAction = true
        row.addArrangedSubview(color)
        let close = UIButton(configuration: .plain())
        close.configuration?.image = UIImage(systemName: "xmark.circle.fill")
        close.configuration?.baseForegroundColor = .secondaryLabel
        close.accessibilityLabel = "Close"
        close.widthAnchor.constraint(equalToConstant: 36).isActive = true
        close.addAction(UIAction { [weak self] _ in self?.onClose?() }, for: .primaryActionTriggered)
        row.addArrangedSubview(close)
        row.spacing = 2
        row.alignment = .center
        // Within a narrow column the tray scrolls rather than hide its end.
        let scroll = UIScrollView()
        scroll.showsHorizontalScrollIndicator = false
        scroll.addSubview(row)
        backdrop.contentView.addSubview(scroll)
        addSubview(backdrop)
        for view in [backdrop, scroll, row] as [UIView] { view.translatesAutoresizingMaskIntoConstraints = false }
        let fit = scroll.widthAnchor.constraint(equalTo: row.widthAnchor)
        fit.priority = .defaultHigh
        NSLayoutConstraint.activate([
            backdrop.leadingAnchor.constraint(equalTo: leadingAnchor),
            backdrop.trailingAnchor.constraint(equalTo: trailingAnchor),
            backdrop.topAnchor.constraint(equalTo: topAnchor),
            backdrop.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: backdrop.contentView.leadingAnchor, constant: 8),
            scroll.trailingAnchor.constraint(equalTo: backdrop.contentView.trailingAnchor, constant: -8),
            scroll.topAnchor.constraint(equalTo: backdrop.contentView.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: backdrop.contentView.bottomAnchor),
            scroll.heightAnchor.constraint(equalToConstant: 64),
            fit,
            row.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            row.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            row.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            row.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            row.heightAnchor.constraint(equalTo: scroll.frameLayoutGuide.heightAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    private var tool: InkTool = .pen(0)

    private func isHighlighter(_ tool: InkTool) -> Bool {
        if case .pen(let place) = tool { pens[Int(place)].highlighter } else { false }
    }

    /// Shows `tool` as the Pencil's, raised, and Select & Type chosen unless a finger draws.
    func show(_ tool: InkTool, fingerDraws: Bool) {
        self.tool = tool
        switch tool {
        case .pen(let place) where isHighlighter(tool): lastHighlighter = place
        case .pen(let place): lastPen = place
        case .shape(let kind): lastShape = kind
        default: break
        }
        let raised: [(UIButton, Bool)] = [
            (select, !fingerDraws),
            (pen, tool == .pen(lastPen)),
            (highlighter, tool == .pen(lastHighlighter)),
            (eraser, tool == .eraser),
            (lasso, tool == .lasso),
            (shape, tool == .shape(lastShape)),
        ]
        for (button, on) in raised { button.isSelected = on }
        pen.configuration?.baseForegroundColor = pens[Int(lastPen)].color ?? .label
        highlighter.configuration?.baseForegroundColor = pens[Int(lastHighlighter)].color
        shape.configuration?.image = UIImage(systemName: Self.shapes[Int(lastShape)].0)
        let shown = pens[Int(isHighlighter(tool) ? lastHighlighter : lastPen)]
        color.configuration?.image = UIImage(systemName: "circle.fill")
        color.configuration?.baseForegroundColor = shown.color ?? .label
        color.accessibilityLabel = "Colour, \(Self.name(shown.color))"
    }

    private func shapeMenu() -> [UIMenuElement] {
        Self.shapes.enumerated().map { place, shape in
            let kind = UInt8(place)
            let action = UIAction(title: shape.1, image: UIImage(systemName: shape.0)) { [weak self] _ in
                self?.onPick?(.shape(kind))
            }
            action.state = tool == .shape(kind) ? .on : .off
            return action
        }
    }

    /// The colours of the kind of pen shown, and for a pen its two widths.
    private func colorMenu() -> [UIMenuElement] {
        let highlighting = isHighlighter(tool)
        let current = highlighting ? lastHighlighter : lastPen
        let width = pens[Int(current)].width
        let colors = pens.indices.filter {
            pens[$0].highlighter == highlighting && (highlighting || pens[$0].width == width)
        }
        let swatches = colors.map { place in
            let color = pens[place].color ?? .label
            let action = UIAction(
                title: Self.name(pens[place].color),
                image: UIImage(systemName: "circle.fill")?.withTintColor(color, renderingMode: .alwaysOriginal)
            ) { [weak self] _ in self?.onPick?(.pen(UInt8(place))) }
            action.state = UInt8(place) == current ? .on : .off
            return action
        }
        guard !highlighting else { return swatches }
        // The same colour in the gallery's other width, where it has one.
        let widths = Set(pens.filter { !$0.highlighter }.map(\.width)).sorted()
        let weights = widths.enumerated().map { step, other in
            let match = pens.indices.first {
                !pens[$0].highlighter && pens[$0].width == other && pens[$0].color == pens[Int(current)].color
            }
            let action = UIAction(
                title: step == 0 ? "Thin" : "Thick", image: UIImage(systemName: step == 0 ? "line.3.horizontal" : "equal"),
                attributes: match == nil ? .disabled : []
            ) { [weak self] _ in if let match { self?.onPick?(.pen(UInt8(match))) } }
            action.state = other == width ? .on : .off
            return action
        }
        return [UIMenu(options: .displayInline, children: swatches), UIMenu(options: .displayInline, children: weights)]
    }

    private static func name(_ color: UIColor?) -> String { color?.accessibilityName.capitalized ?? "Black" }

    private static func tool(_ symbol: String, _ label: String) -> UIButton {
        var configuration = UIButton.Configuration.plain()
        configuration.image = UIImage(systemName: symbol)
        configuration.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(pointSize: 24)
        configuration.baseForegroundColor = .label
        configuration.cornerStyle = .medium
        let button = UIButton(configuration: configuration)
        button.accessibilityLabel = label
        button.widthAnchor.constraint(equalToConstant: 42).isActive = true
        button.heightAnchor.constraint(equalToConstant: 52).isActive = true
        // The chosen tool stands up out of the tray, as PencilKit's does.
        button.configurationUpdateHandler = { button in
            button.transform = CGAffineTransform(translationX: 0, y: button.isSelected ? -5 : 4)
            button.configuration?.background.backgroundColor =
                button.isSelected ? UIColor.tintColor.withAlphaComponent(0.14) : .clear
        }
        return button
    }

    private static func divider() -> UIView {
        let line = UIView()
        line.backgroundColor = .separator
        line.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            line.widthAnchor.constraint(equalToConstant: 1),
            line.heightAnchor.constraint(equalToConstant: 32),
        ])
        return line
    }
}
