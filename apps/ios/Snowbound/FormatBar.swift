import UIKit

/// Formatting above the keyboard, as Notes offers it: To Do first, then text styles, lists,
/// indentation, tags and pictures, the keyboard's dismissal at the end.
final class FormatBar: UIInputView {
    /// `sb_view_apply` commands and `sb_view_format` bits.
    private static let buttons: [(symbol: String, label: String, command: UInt8)] = [
        ("checklist", "To Do", 16),
        ("bold", "Bold", 0),
        ("italic", "Italic", 1),
        ("underline", "Underline", 2),
        ("strikethrough", "Strikethrough", 3),
        ("highlighter", "Highlight", 10),
        ("list.bullet", "Bullets", 4),
        ("list.number", "Numbering", 5),
        ("decrease.indent", "Decrease Indent", 7),
        ("increase.indent", "Increase Indent", 6),
    ]
    private var toggles: [(UIButton, UInt8)] = []
    private var bits: UInt64 = 0
    var onApply: ((UInt8) -> Void)?
    /// Asks for a picture from the camera (true) or the photo library.
    var onPicture: ((Bool) -> Void)?
    var onDismiss: (() -> Void)?

    init() {
        super.init(frame: CGRect(x: 0, y: 0, width: 0, height: 48), inputViewStyle: .keyboard)
        allowsSelfSizing = true
        // Over the page when a hardware keyboard hides the software one.
        let backdrop = UIVisualEffectView(effect: UIBlurEffect(style: .systemChromeMaterial))
        backdrop.frame = bounds
        backdrop.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(backdrop)
        let row = UIStackView()
        row.spacing = 2
        for (symbol, label, command) in Self.buttons {
            let button = Self.button(symbol, label)
            button.addAction(UIAction { [weak self] _ in self?.onApply?(command) }, for: .primaryActionTriggered)
            toggles.append((button, command))
            row.addArrangedSubview(button)
        }
        let tag = Self.button("tag", "Tags")
        tag.menu = UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak self] done in
                done(Tags.menu(bits: self?.bits ?? 0) { self?.onApply?($0) })
            }
        ])
        tag.showsMenuAsPrimaryAction = true
        row.addArrangedSubview(tag)
        let picture = Self.button("photo", "Insert Picture")
        picture.menu = UIMenu(children: [
            UIAction(title: "Photo Library", image: UIImage(systemName: "photo.on.rectangle")) { [weak self] _ in
                self?.onPicture?(false)
            },
            UIAction(
                title: "Take Photo", image: UIImage(systemName: "camera"),
                attributes: UIImagePickerController.isSourceTypeAvailable(.camera) ? [] : .disabled
            ) { [weak self] _ in self?.onPicture?(true) },
        ])
        picture.showsMenuAsPrimaryAction = true
        row.addArrangedSubview(picture)
        let scroll = UIScrollView()
        scroll.showsHorizontalScrollIndicator = false
        scroll.addSubview(row)
        let dismiss = Self.button("keyboard.chevron.compact.down", "Hide Keyboard")
        dismiss.addAction(UIAction { [weak self] _ in self?.onDismiss?() }, for: .primaryActionTriggered)
        for view in [scroll, dismiss, row] as [UIView] { view.translatesAutoresizingMaskIntoConstraints = false }
        addSubview(scroll)
        addSubview(dismiss)
        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: 48),
            scroll.leadingAnchor.constraint(equalTo: safeAreaLayoutGuide.leadingAnchor, constant: 4),
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroll.trailingAnchor.constraint(equalTo: dismiss.leadingAnchor, constant: -4),
            dismiss.trailingAnchor.constraint(equalTo: safeAreaLayoutGuide.trailingAnchor, constant: -4),
            dismiss.centerYAnchor.constraint(equalTo: centerYAnchor),
            row.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            row.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            row.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            row.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            row.heightAnchor.constraint(equalTo: scroll.frameLayoutGuide.heightAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    private static func button(_ symbol: String, _ label: String) -> UIButton {
        var configuration = UIButton.Configuration.plain()
        configuration.image = UIImage(systemName: symbol)
        configuration.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(pointSize: 17)
        configuration.baseForegroundColor = .label
        configuration.cornerStyle = .medium
        let button = UIButton(configuration: configuration)
        button.accessibilityLabel = label
        button.widthAnchor.constraint(equalToConstant: 42).isActive = true
        button.configurationUpdateHandler = { button in
            button.configuration?.background.backgroundColor =
                button.isSelected ? UIColor.tintColor.withAlphaComponent(0.18) : .clear
            button.configuration?.baseForegroundColor = button.isSelected ? .tintColor : .label
        }
        return button
    }

    /// Marks what the selection has, from `sb_view_format`.
    func show(_ bits: UInt64) {
        self.bits = bits
        for (button, command) in toggles {
            button.isSelected = bits & (1 << UInt64(command)) != 0
        }
    }
}
