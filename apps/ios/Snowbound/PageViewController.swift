import PhotosUI
import UIKit

final class PageViewController: UIViewController, PHPickerViewControllerDelegate,
    UIImagePickerControllerDelegate, UINavigationControllerDelegate
{
    /// Posted when the app leaves the screen or quits: marked text is committed so it saves.
    static let leaving = Notification.Name("SnowboundLeaving")

    private let section: Section
    private let page: String
    private let find: String?
    private let titleFocus: Bool
    private lazy var canvas = CanvasView(section: section, page: page)
    private let bar = ConflictBar()
    /// Opens another page or conflict page of the section.
    var onOpen: ((String) -> Void)?
    private lazy var done = UIBarButtonItem(
        systemItem: .done, primaryAction: UIAction { [weak self] _ in _ = self?.canvas.resignFirstResponder() })
    private lazy var undo = UIBarButtonItem(
        systemItem: .undo, primaryAction: UIAction { [weak self] _ in self?.canvas.undo(redo: false) })
    private lazy var redo = UIBarButtonItem(
        systemItem: .redo, primaryAction: UIAction { [weak self] _ in self?.canvas.undo(redo: true) })

    init(section: Section, page: String, find: String? = nil, titleFocus: Bool = false) {
        self.section = section
        self.page = page
        self.find = find
        self.titleFocus = titleFocus
        super.init(nibName: nil, bundle: nil)
        navigationItem.largeTitleDisplayMode = .never
        showTitle(section.row(of: page)?.row.title ?? "")
    }

    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        let view = UIView()
        view.backgroundColor = .systemBackground
        canvas.translatesAutoresizingMaskIntoConstraints = false
        bar.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(canvas)
        view.addSubview(bar)
        NSLayoutConstraint.activate([
            canvas.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            canvas.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            canvas.topAnchor.constraint(equalTo: view.topAnchor),
            canvas.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            bar.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            bar.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
        ])
        self.view = view
        canvas.onChange = { [weak self] in self?.editingChanged() }
        canvas.onOpened = { [weak self] in self?.opened() }
        canvas.onDate = { [weak self] field, date in self?.pickDate(field, date) }
        canvas.formatBar.onPicture = { [weak self] camera in self?.pickPicture(camera: camera) }
        bar.onAction = { [weak self] in self?.conflictAction() }
        NotificationCenter.default.addObserver(
            self, selector: #selector(sectionChanged), name: Section.changed, object: section)
        NotificationCenter.default.addObserver(self, selector: #selector(leaving), name: Self.leaving, object: nil)
        conflictChanged()
    }

    /// The page is on screen: a found word is selected, a new page's title takes the caret.
    private func opened() {
        if let find, canvas.find(find) {
            _ = canvas.becomeFirstResponder()
        } else if titleFocus, canvas.focusTitle() {
            _ = canvas.becomeFirstResponder()
        }
    }

    private func showTitle(_ title: String) {
        self.title = title.isEmpty ? "Untitled Page" : title
    }

    /// Notes' checkmark ends editing and puts the keyboard away; undo and redo sit beside it.
    private func editingChanged() {
        if !canvas.readOnly, let title = canvas.pageTitle {
            showTitle(title)
            section.retitle(page, title)
        }
        guard canvas.isFirstResponder else {
            navigationItem.rightBarButtonItems = nil
            return
        }
        undo.isEnabled = canvas.canUndo(redo: false)
        redo.isEnabled = canvas.canUndo(redo: true)
        if navigationItem.rightBarButtonItems?.first !== done {
            navigationItem.rightBarButtonItems = [done, redo, undo]
        }
    }

    @objc private func leaving() { canvas.commitComposition() }

    @objc private func sectionChanged(_ notification: Notification) {
        let flags = notification.userInfo?["flags"] as? UInt32 ?? 0
        if flags & Section.rejected != 0 {
            // Nothing typed is lost unannounced: the page's text goes on the clipboard.
            UIPasteboard.general.string = canvas.pageText
            canvas.reload(discard: true)
            let alert = UIAlertController(
                title: "Change Not Saved",
                message: "The page shows what was last saved. Your text is on the clipboard to paste back.",
                preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "OK", style: .default))
            present(alert, animated: true)
        } else if flags & Section.remote != 0 {
            canvas.reload(discard: false)
        }
        if flags & Section.listed != 0 {
            if section.row(of: page) == nil {
                // Deleted elsewhere, or a conflict page resolved: back to what the section lists.
                if let first = section.rows.first { onOpen?(first.id) }
                return
            }
            conflictChanged()
        }
        showStatus()
    }

    /// Only trouble shows: saving goes on quietly, as Notes saves.
    private func showStatus() {
        let status: String? =
            switch section.status {
            case .offline: "Offline"
            case .notSaving: "Not Saving"
            default: nil
            }
        if #available(iOS 26, *) {
            navigationItem.subtitle = status
        } else {
            navigationItem.prompt = status
        }
    }

    // MARK: Conflicts

    private func conflictChanged() {
        guard let (row, version) = section.row(of: page) else { return bar.show(nil) }
        if version != nil {
            // Deleting a conflict page cannot be undone, so the button asks first.
            let delete = UIAction(
                title: "Delete Conflict Page", image: UIImage(systemName: "trash"), attributes: .destructive
            ) { [weak self] _ in self?.conflictAction() }
            bar.show(
                "Changes in red couldn’t be merged. This version can’t be edited.", action: "Delete",
                menu: UIMenu(children: [delete]))
        } else if !row.versions.isEmpty {
            bar.show("This page has changes that couldn’t be merged.", action: "See Version")
        } else {
            bar.show(nil)
        }
        canvas.topInset = bar.isHidden ? 0 : bar.systemLayoutSizeFitting(view.bounds.size).height
    }

    private func conflictAction() {
        guard let (row, version) = section.row(of: page) else { return }
        if version != nil {
            section.delete(page) { [weak self] _ in self?.onOpen?(row.id) }
        } else if let newest = row.versions.max(by: { ($0.created ?? 0) < ($1.created ?? 0) }) {
            onOpen?(newest.id)
        }
    }

    // MARK: Date

    /// Changes the page's date or time, as OneNote's date and time under the title do.
    private func pickDate(_ field: DateField, _ shown: Date) {
        let picker = UIDatePicker()
        picker.datePickerMode = field == .date ? .date : .time
        picker.preferredDatePickerStyle = field == .date ? .inline : .wheels
        picker.date = shown
        picker.minimumDate = Date(timeIntervalSince1970: -11_644_473_600)
        picker.accessibilityLabel = field == .date ? "Page date" : "Page time"
        let controller = UIViewController()
        controller.title = field == .date ? "Change Page Date" : "Change Page Time"
        picker.translatesAutoresizingMaskIntoConstraints = false
        controller.view.backgroundColor = .systemBackground
        controller.view.addSubview(picker)
        NSLayoutConstraint.activate([
            picker.centerXAnchor.constraint(equalTo: controller.view.centerXAnchor),
            picker.topAnchor.constraint(equalTo: controller.view.safeAreaLayoutGuide.topAnchor, constant: 8),
            picker.widthAnchor.constraint(lessThanOrEqualTo: controller.view.widthAnchor, constant: -32),
        ])
        controller.navigationItem.leftBarButtonItem = UIBarButtonItem(
            systemItem: .cancel, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        controller.navigationItem.rightBarButtonItem = UIBarButtonItem(
            title: "Apply",
            primaryAction: UIAction { [weak self] _ in
                // The field not being changed keeps what the page had.
                let calendar = Calendar.current
                let kept: Set<Calendar.Component> = field == .date ? [.hour, .minute, .second] : [.era, .year, .month, .day]
                let changed: Set<Calendar.Component> = field == .date ? [.era, .year, .month, .day] : [.hour, .minute]
                var components = calendar.dateComponents(kept, from: shown)
                let chosen = calendar.dateComponents(changed, from: picker.date)
                for component in changed { components.setValue(chosen.value(for: component), for: component) }
                if field == .time { components.second = calendar.component(.second, from: shown) }
                self?.dismiss(animated: true)
                if let date = calendar.date(from: components) { self?.canvas.changeDate(date) }
            })
        let navigation = UINavigationController(rootViewController: controller)
        navigation.sheetPresentationController?.detents = field == .date ? [.large()] : [.medium()]
        present(navigation, animated: true)
    }

    // MARK: Pictures

    private func pickPicture(camera: Bool) {
        if camera {
            guard UIImagePickerController.isSourceTypeAvailable(.camera) else { return }
            let picker = UIImagePickerController()
            picker.sourceType = .camera
            picker.delegate = self
            present(picker, animated: true)
        } else {
            var configuration = PHPickerConfiguration()
            configuration.filter = .images
            let picker = PHPickerViewController(configuration: configuration)
            picker.delegate = self
            present(picker, animated: true)
        }
    }

    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        picker.dismiss(animated: true)
        guard let provider = results.first?.itemProvider, provider.canLoadObject(ofClass: UIImage.self) else { return }
        provider.loadObject(ofClass: UIImage.self) { [weak self] image, _ in
            guard let image = image as? UIImage else { return }
            DispatchQueue.main.async { self?.insert(image) }
        }
    }

    func imagePickerController(
        _ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
    ) {
        picker.dismiss(animated: true)
        if let image = info[.originalImage] as? UIImage { insert(image) }
    }

    /// A photo as OneNote 2010 can read it: JPEG, at most 2048 pixels across, shown at most
    /// 220 points wide.
    private func insert(_ image: UIImage) {
        let longest: CGFloat = 2048
        let scale = min(1, longest / max(image.size.width * image.scale, image.size.height * image.scale))
        let pixels = CGSize(
            width: (image.size.width * image.scale * scale).rounded(),
            height: (image.size.height * image.scale * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let drawn = UIGraphicsImageRenderer(size: pixels, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: pixels))
        }
        guard let data = drawn.jpegData(compressionQuality: 0.85) else { return }
        // OneNote shows a picture's pixels at 96 per inch.
        var size = CGSize(width: pixels.width * 0.75, height: pixels.height * 0.75)
        if size.width > 220 { size = CGSize(width: 220, height: size.height * 220 / size.width) }
        canvas.insertPicture(data, size: size)
    }
}

/// OneNote's information bar above a page whose changes could not all be merged.
final class ConflictBar: UIView {
    private let label = UILabel()
    private let button = UIButton(configuration: .bordered())
    var onAction: (() -> Void)?

    init() {
        super.init(frame: .zero)
        backgroundColor = UIColor { $0.userInterfaceStyle == .dark ? UIColor(red: 0.35, green: 0.28, blue: 0.1, alpha: 1) : UIColor(red: 1, green: 0.93, blue: 0.76, alpha: 1) }
        let icon = UIImageView(image: UIImage(systemName: "exclamationmark.triangle.fill"))
        icon.tintColor = .systemOrange
        icon.setContentHuggingPriority(.required, for: .horizontal)
        label.font = .preferredFont(forTextStyle: .footnote)
        label.numberOfLines = 0
        button.addAction(
            UIAction { [weak self] _ in
                if self?.button.menu == nil { self?.onAction?() }
            }, for: .primaryActionTriggered)
        button.setContentHuggingPriority(.required, for: .horizontal)
        let stack = UIStackView(arrangedSubviews: [icon, label, button])
        stack.spacing = 10
        stack.alignment = .center
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: layoutMarginsGuide.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: layoutMarginsGuide.trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -8),
        ])
        isHidden = true
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Shows `message` with a button titled `action`, which opens `menu` when there is one.
    func show(_ message: String?, action: String = "", menu: UIMenu? = nil) {
        isHidden = message == nil
        label.text = message
        button.configuration?.title = action
        button.menu = menu
        button.showsMenuAsPrimaryAction = menu != nil
    }
}
