import PhotosUI
import UIKit

/// What a page shows selected as it opens.
enum Reveal {
    /// The first match of a search.
    case text(String)
    /// A paragraph the Tags Summary lists, by id.
    case paragraph(String)
}

final class PageViewController: UIViewController, PHPickerViewControllerDelegate,
    UIImagePickerControllerDelegate, UINavigationControllerDelegate
{
    /// Posted when the app leaves the screen or quits: marked text is committed so it saves.
    static let leaving = Notification.Name("SnowboundLeaving")

    private let section: Section
    private let page: String
    private let reveal: Reveal?
    private let titleFocus: Bool
    private lazy var canvas = CanvasView(section: section, page: page)
    private let bar = ConflictBar()
    /// What records or plays on this page, over its foot.
    private let media = MediaBar()
    private var playback: Playback?
    private var mediaClock: Timer?
    /// Reading View, beside the page menu where the page has one.
    private var reader: UIBarButtonItem?
    /// Opens another page or conflict page of the section.
    var onOpen: ((String) -> Void)?
    private lazy var done = UIBarButtonItem(
        systemItem: .done, primaryAction: UIAction { [weak self] _ in _ = self?.canvas.resignFirstResponder() })
    private lazy var undo = UIBarButtonItem(
        systemItem: .undo, primaryAction: UIAction { [weak self] _ in self?.canvas.undo(redo: false) })
    private lazy var redo = UIBarButtonItem(
        systemItem: .redo, primaryAction: UIAction { [weak self] _ in self?.canvas.undo(redo: true) })
    /// Shows the drawing tools, which a finger then draws with too.
    private lazy var draw = UIBarButtonItem(
        title: "Draw", image: UIImage(systemName: "pencil.tip.crop.circle"),
        primaryAction: UIAction { [weak self] _ in
            guard let canvas = self?.canvas else { return }
            canvas.showPicker(!canvas.pickerShown)
        })
    private lazy var more = UIBarButtonItem(
        title: "Page", image: UIImage(systemName: "ellipsis.circle"),
        menu: UIMenu(children: [UIDeferredMenuElement.uncached { [weak self] done in done(self?.pageMenu() ?? []) }]))

    init(section: Section, page: String, reveal: Reveal? = nil, titleFocus: Bool = false) {
        self.section = section
        self.page = page
        self.reveal = reveal
        self.titleFocus = titleFocus
        super.init(nibName: nil, bundle: nil)
        navigationItem.largeTitleDisplayMode = .never
        navigationItem.rightBarButtonItems = [more, draw]
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
        media.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(media)
        let wide = media.widthAnchor.constraint(equalToConstant: 480)
        wide.priority = .required - 1
        NSLayoutConstraint.activate([
            wide,
            media.centerXAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerXAnchor),
            media.leadingAnchor.constraint(greaterThanOrEqualTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 12),
            media.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -12),
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
        canvas.onPlay = { [weak self] request in self?.play(request) }
        media.onPause = { [weak self] in self?.pauseMedia() }
        media.onStop = { [weak self] in self?.stopMedia() }
        NotificationCenter.default.addObserver(self, selector: #selector(showMedia), name: Recorder.changed, object: nil)
        canvas.formatBar.onPicture = { [weak self] camera in self?.pickPicture(camera: camera) }
        bar.onAction = { [weak self] in self?.conflictAction() }
        NotificationCenter.default.addObserver(
            self, selector: #selector(sectionChanged), name: Section.changed, object: section)
        NotificationCenter.default.addObserver(self, selector: #selector(leaving), name: Self.leaving, object: nil)
        conflictChanged()
        registerForTraitChanges([UITraitHorizontalSizeClass.self]) { (controller: PageViewController, _) in
            controller.fitTitle()
        }
    }

    /// The page is on screen: a found word or tagged paragraph is selected, a new page's
    /// title takes the caret.
    private func opened() {
        let revealed =
            switch reveal {
            case .text(let query): canvas.find(query)
            case .paragraph(let id): canvas.selectParagraph(id)
            case nil: false
            }
        if revealed || titleFocus && canvas.focusTitle() {
            _ = canvas.becomeFirstResponder()
        }
        if Prototype.reading, canvas.reading?.offered == true {
            reader = UIBarButtonItem(
                title: "Reading View", image: UIImage(systemName: "text.justify.left"),
                primaryAction: UIAction { [weak self] _ in self?.toggleReading() })
            editingChanged()
        }
    }

    private func showTitle(_ title: String) {
        self.title = title.isEmpty ? "Untitled Page" : title
    }

    /// A phone's bar has no room for the title beside the editing buttons, and the page
    /// shows it anyway, as Notes leaves it out.
    private func fitTitle() {
        let crowded = canvas.isFirstResponder && traitCollection.horizontalSizeClass == .compact
        navigationItem.titleView = crowded ? UIView() : nil
    }

    /// Notes' checkmark ends editing and puts the keyboard away; undo and redo sit beside it.
    private func editingChanged() {
        if !canvas.readOnly, let title = canvas.pageTitle {
            showTitle(title)
            section.retitle(page, title)
        }
        fitTitle()
        draw.isSelected = canvas.pickerShown
        draw.isEnabled = !canvas.readOnly
        undo.isEnabled = canvas.canUndo(redo: false)
        redo.isEnabled = canvas.canUndo(redo: true)
        // Drawing keeps undo at hand, as typing does.
        reader?.isSelected = canvas.readingShown
        guard canvas.isFirstResponder || canvas.pickerShown else {
            let resting = [more, draw] + (reader.map { [$0] } ?? [])
            if navigationItem.rightBarButtonItems != resting { navigationItem.rightBarButtonItems = resting }
            return
        }
        let items = canvas.isFirstResponder ? [done, more, draw, redo, undo] : [more, draw, redo, undo]
        if navigationItem.rightBarButtonItems?.first !== items.first
            || navigationItem.rightBarButtonItems?.count != items.count
        {
            navigationItem.rightBarButtonItems = items
        }
    }

    // MARK: Page menu

    /// The desktop's page and insert commands that suit a touch screen.
    private func pageMenu() -> [UIMenuElement] {
        let editable: UIMenuElement.Attributes = canvas.readOnly ? .disabled : []
        let scene = view.window?.windowScene?.delegate as? SceneDelegate
        let insert = UIMenu(
            title: "Insert", image: UIImage(systemName: "plus.circle"),
            children: [
                UIAction(title: "Photo Library", image: UIImage(systemName: "photo.on.rectangle"), attributes: editable) {
                    [weak self] _ in self?.pickPicture(camera: false)
                },
                UIAction(
                    title: "Take Photo", image: UIImage(systemName: "camera"),
                    attributes: UIImagePickerController.isSourceTypeAvailable(.camera) ? editable : .disabled
                ) { [weak self] _ in self?.pickPicture(camera: true) },
                UIMenu(options: .displayInline, children: [
                    UIAction(title: "Date", image: UIImage(systemName: "calendar"), attributes: editable) {
                        [weak self] _ in self?.canvas.insertDate(true, time: false)
                    },
                    UIAction(title: "Time", image: UIImage(systemName: "clock"), attributes: editable) {
                        [weak self] _ in self?.canvas.insertDate(false, time: true)
                    },
                    UIAction(title: "Date & Time", image: UIImage(systemName: "calendar.badge.clock"), attributes: editable) {
                        [weak self] _ in self?.canvas.insertDate(true, time: true)
                    },
                ]),
                UIAction(
                    title: "Space", image: UIImage(systemName: "arrow.up.and.down.text.horizontal"), attributes: editable
                ) { [weak self] _ in self?.canvas.insertSpace() },
                UIMenu(options: .displayInline, children: [
                    UIAction(
                        title: "Record Audio", image: UIImage(systemName: "mic"),
                        attributes: Recorder.current == nil ? editable : .disabled
                    ) { [weak self] _ in self?.record(video: false) },
                    UIAction(
                        title: "Record Video", image: UIImage(systemName: "video"),
                        attributes: Recorder.current == nil && Recorder.canRecordVideo ? editable : .disabled
                    ) { [weak self] _ in self?.record(video: true) },
                ]),
            ])
        return (Prototype.reading ? [readingAction()] : []) + [
            insert,
            UIAction(title: "Page Background…", image: UIImage(systemName: "paintpalette"), attributes: editable) {
                [weak self] _ in self?.showPaper()
            },
            themeMenu(),
            UIAction(title: "Tags Summary", image: UIImage(systemName: "tag")) { [weak self] _ in
                guard let self else { return }
                scene?.showTags(of: section.notebook, from: self)
            },
            UIMenu(options: .displayInline, children: [
                UIAction(title: "Print…", image: UIImage(systemName: "printer")) { [weak self] _ in
                    self?.printPage(export: false)
                },
                UIAction(title: "Export as PDF…", image: UIImage(systemName: "doc.richtext")) { [weak self] _ in
                    self?.printPage(export: true)
                },
            ]),
            UIMenu(options: .displayInline, children: [
                UIAction(title: "New Subpage", image: UIImage(systemName: "text.badge.plus")) { _ in
                    scene?.newPage(subpage: true)
                },
                UIAction(title: "Delete Page", image: UIImage(systemName: "trash"), attributes: .destructive) {
                    [weak self] _ in
                    guard let self else { return }
                    section.delete(page)
                },
            ]),
        ]
    }

    /// Reading View: the page reflowed into the screen's width, read-only, where it reflows.
    private func readingAction() -> UIAction {
        let reading = canvas.reading
        return UIAction(
            title: "Reading View", subtitle: reading.flatMap { Prototype.readingNote($0.verdict) },
            image: UIImage(systemName: "text.justify.left"),
            attributes: reading?.offered == true || canvas.readingShown ? [] : .disabled,
            state: canvas.readingShown ? .on : .off
        ) { [weak self] _ in self?.toggleReading() }
    }

    private func toggleReading() {
        // A recording goes on the page as laid out.
        guard Recorder.current?.canvas !== canvas else { return }
        canvas.showReading(!canvas.readingShown)
    }

    /// The page's theme, its section's and its notebook's, each pickable; the page wears a
    /// new one when it opens again.
    private func themeMenu() -> UIMenu {
        guard let themes = section.themes(of: page) else {
            return UIMenu(title: "Theme", image: UIImage(systemName: "textformat"), children: [])
        }
        let scopes: [(String, String?)] = [
            ("This Page", themes.page), ("This Section", themes.section), ("This Notebook", themes.notebook),
        ]
        let page = page
        let menus = scopes.enumerated().map { scope, entry in
            let (title, current) = entry
            let choices = [(nil as String?, "None")] + themes.themes.map { ($0.id, $0.name) }
            return UIMenu(title: title, children: choices.map { id, name in
                UIAction(title: name, state: id == current ? .on : .off) { [weak self] _ in
                    guard let self else { return }
                    section.setTheme(id, scope: UInt8(scope), of: page) { [weak self] _ in
                        self?.onOpen?(page)
                    }
                }
            })
        }
        return UIMenu(title: "Theme", image: UIImage(systemName: "textformat"), children: menus)
    }

    /// Prints the page through the system's print sheet, or with `export` offers its PDF in
    /// the share sheet, laid out as OneNote 2010 prints it on the region's paper.
    private func printPage(export: Bool) {
        let letter = ["US", "CA", "MX", "PR", "PH", "CL", "CO", "VE", "GT", "CR", "PA", "DO", "SV", "NI", "HN", "BZ"]
            .contains(Locale.current.region?.identifier ?? "US")
        let paper = letter ? CGSize(width: 612, height: 792) : CGSize(width: 595.276, height: 841.89)
        let title = canvas.pageTitle ?? section.row(of: page)?.row.title ?? ""
        guard let pdf = canvas.pdf(paper: paper, section: section.tab.name) else {
            return alert(
                export ? "Couldn't Export the PDF" : "Couldn't Print", "The page could not be laid out on paper.")
        }
        if export {
            let name = title.components(separatedBy: CharacterSet(charactersIn: "/\\:")).joined(separator: "-")
                .trimmingCharacters(in: .whitespaces)
            let file = FileManager.default.temporaryDirectory
                .appendingPathComponent((name.isEmpty ? "Untitled" : name) + ".pdf")
            do {
                try pdf.write(to: file)
            } catch {
                return
            }
            let share = UIActivityViewController(activityItems: [file], applicationActivities: nil)
            share.popoverPresentationController?.barButtonItem = more
            present(share, animated: true)
        } else {
            let info = UIPrintInfo.printInfo()
            info.jobName = title
            let printer = UIPrintInteractionController.shared
            printer.printInfo = info
            printer.printingItem = pdf
            printer.present(from: more, animated: true)
        }
    }

    private func showPaper() {
        let navigation = UINavigationController(rootViewController: PaperViewController(canvas: canvas))
        navigation.sheetPresentationController?.detents = [.medium(), .large()]
        navigation.sheetPresentationController?.prefersGrabberVisible = true
        navigation.sheetPresentationController?.largestUndimmedDetentIdentifier = .medium
        present(navigation, animated: true)
    }

    @objc private func leaving() { canvas.commitComposition() }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        // A recording goes on the page it started on, as the desktop's does on leaving it.
        if Recorder.current?.canvas === canvas { Recorder.current?.stop() }
        stopPlayback()
    }

    // MARK: Recording and playback

    func record(video: Bool) {
        stopPlayback()
        Recorder.start(video: video, on: canvas) { [weak self] title, message in self?.alert(title, message) }
    }

    func stopRecording() {
        if Recorder.current?.canvas === canvas { Recorder.current?.stop() }
    }

    private func play(_ request: PlayRequest) {
        guard Recorder.current == nil else { return }
        playback = Playback(request)
        showMedia()
    }

    private func stopPlayback() {
        guard playback != nil else { return }
        playback = nil
        canvas.played(at: nil)
        showMedia()
    }

    private func pauseMedia() {
        if let recorder = Recorder.current, recorder.canvas === canvas {
            recorder.pause(!(recorder.elapsed?.paused ?? false))
        } else {
            playback?.toggle()
        }
        showMedia()
    }

    private func stopMedia() {
        if Recorder.current?.canvas === canvas {
            Recorder.current?.stop()
        } else {
            stopPlayback()
        }
    }

    /// The bar shows the clock of what records or plays, and a video's picture as it plays.
    @objc private func showMedia() {
        if let recorder = Recorder.current, recorder.canvas === canvas {
            media.picture.isHidden = true
            if recorder.saving {
                media.showSaving(recorder.video ? "Saving video…" : "Saving audio…")
            } else if let elapsed = recorder.elapsed {
                let state = elapsed.paused ? "Paused" : recorder.video ? "Recording video" : "Recording"
                media.show(
                    "\(state)  \(MediaBar.clock(elapsed.ms))", recording: true,
                    paused: recorder.pausable ? elapsed.paused : nil)
            }
        } else if let playback {
            if playback.failed {
                stopPlayback()
                return alert("Couldn't Play the Recording", "This kind of recording doesn't play on this device.")
            }
            let total = playback.durationMs.map { " / " + MediaBar.clock($0) } ?? ""
            media.show(
                "\(playback.request.name)  \(MediaBar.clock(playback.ms))\(total)", recording: false,
                paused: !playback.playing, playing: true)
            if let picture = playback.picture() { media.picture.image = picture }
            media.picture.isHidden = playback.request.video == nil
            canvas.played(at: playback.ms)
        } else {
            media.isHidden = true
            canvas.bottomInset = 0
            mediaClock?.invalidate()
            mediaClock = nil
            return
        }
        // The line being written stays above the bar.
        canvas.bottomInset = view.keyboardLayoutGuide.layoutFrame.minY - media.frame.minY
        if mediaClock == nil {
            mediaClock = Timer.scheduledTimer(withTimeInterval: 1.0 / 15, repeats: true) { [weak self] _ in
                self?.showMedia()
            }
        }
    }

    @objc private func sectionChanged(_ notification: Notification) {
        let flags = notification.userInfo?["flags"] as? UInt32 ?? 0
        if flags & Section.rejected != 0 {
            // Nothing typed is lost unannounced: the page's text goes on the clipboard.
            UIPasteboard.general.string = canvas.pageText
            canvas.reload(discard: true)
            alert("Change Not Saved", "The page shows what was last saved. Your text is on the clipboard to paste back.")
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
            section.delete(page) { [weak self] in self?.onOpen?(row.id) }
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
            DispatchQueue.main.async { self?.canvas.insertPicture(image) }
        }
    }

    func imagePickerController(
        _ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
    ) {
        picker.dismiss(animated: true)
        if let image = info[.originalImage] as? UIImage { canvas.insertPicture(image) }
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
