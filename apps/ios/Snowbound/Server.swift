import UIKit

/// Connect to Server, as Files and the Finder offer it: an `smb://` address, then a guest
/// or an account whose password goes in the Keychain, then a folder to open.
final class ServerViewController: UITableViewController, UITextFieldDelegate {
    private static let lastKey = "server"

    private let address = UITextField()
    private let name = UITextField()
    private let password = UITextField()
    private let guest = UISegmentedControl(items: ["Guest", "Registered User"])
    private var connecting = false
    var onOpen: ((Notebook) -> Void)?

    init() {
        super.init(style: .insetGrouped)
        title = "Connect to Server"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        navigationItem.leftBarButtonItem = UIBarButtonItem(
            systemItem: .cancel, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        navigationItem.rightBarButtonItem = UIBarButtonItem(
            title: "Connect", primaryAction: UIAction { [weak self] _ in self?.connect() })
        address.placeholder = "smb://server/share"
        address.text = UserDefaults.standard.string(forKey: Self.lastKey)
        address.keyboardType = .URL
        address.autocapitalizationType = .none
        address.autocorrectionType = .no
        address.clearButtonMode = .whileEditing
        name.placeholder = "Name"
        name.textContentType = .username
        name.autocapitalizationType = .none
        name.autocorrectionType = .no
        password.placeholder = "Password"
        password.textContentType = .password
        password.isSecureTextEntry = true
        for field in [address, name, password] {
            field.delegate = self
            field.returnKeyType = field === password || field === address ? .go : .next
        }
        guest.selectedSegmentIndex = 1
        guest.addAction(UIAction { [weak self] _ in self?.tableView.reloadData() }, for: .valueChanged)
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "field")
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        address.becomeFirstResponder()
    }

    func textFieldShouldReturn(_ field: UITextField) -> Bool {
        if field === name {
            password.becomeFirstResponder()
        } else {
            connect()
        }
        return false
    }

    private var registered: Bool { guest.selectedSegmentIndex == 1 }

    /// `smb://user@host:port/share/folder`, or the same without a scheme.
    private func server() -> Server? {
        var text = address.text?.trimmingCharacters(in: .whitespaces) ?? ""
        if !text.contains("://") { text = "smb://" + text }
        guard let url = URLComponents(string: text), url.scheme?.lowercased() == "smb", let host = url.host,
            !host.isEmpty
        else { return nil }
        let parts = url.path.split(separator: "/").map(String.init)
        guard let share = parts.first else { return nil }
        let user = url.user ?? (registered ? name.text ?? "" : "")
        return Server(
            host: url.port.map { "\(host):\($0)" } ?? host, share: share, user: user,
            root: parts.dropFirst().joined(separator: "/"))
    }

    private func connect() {
        guard !connecting else { return }
        guard let server = server() else {
            return alert("Enter a Server Address", "Use the form smb://server/share.")
        }
        let secret = registered ? password.text ?? "" : ""
        connecting = true
        navigationItem.rightBarButtonItem?.isEnabled = false
        let spinner = UIActivityIndicatorView(style: .medium)
        spinner.startAnimating()
        navigationItem.titleView = spinner
        background({ () -> (Int, String?) in
            var error: UnsafeMutablePointer<CChar>?
            let share = sb_share_connect(server.host, server.share, server.user, secret, "", &error)
            return (Int(bitPattern: share), take(error))
        }) { [weak self] share, _ in
            guard let self else { return }
            connecting = false
            navigationItem.rightBarButtonItem?.isEnabled = true
            navigationItem.titleView = nil
            guard let share = OpaquePointer(bitPattern: share) else {
                return alert(
                    "Can’t Connect", "Check the address, name and password, and that the server is on this network.")
            }
            if registered { Keychain.save(secret, for: server) }
            UserDefaults.standard.set(address.text, forKey: Self.lastKey)
            let browser = ShareViewController(share: SharedConnection(share), server: server, path: server.root)
            browser.onOpen = onOpen
            navigationController?.pushViewController(browser, animated: true)
        }
    }

    override func numberOfSections(in tableView: UITableView) -> Int { registered ? 2 : 1 }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { 2 }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "field", for: indexPath)
        cell.contentView.subviews.forEach { $0.removeFromSuperview() }
        cell.selectionStyle = .none
        let view: UIView =
            switch (indexPath.section, indexPath.row) {
            case (0, 0): address
            case (0, _): guest
            case (_, 0): name
            default: password
            }
        view.translatesAutoresizingMaskIntoConstraints = false
        cell.contentView.addSubview(view)
        let margins = cell.contentView.layoutMarginsGuide
        NSLayoutConstraint.activate([
            view.leadingAnchor.constraint(equalTo: margins.leadingAnchor),
            view.trailingAnchor.constraint(equalTo: margins.trailingAnchor),
            view.topAnchor.constraint(equalTo: margins.topAnchor),
            view.bottomAnchor.constraint(equalTo: margins.bottomAnchor),
            view.heightAnchor.constraint(greaterThanOrEqualToConstant: 32),
        ])
        return cell
    }
}

/// A share connection the browser's folders share, closed with the last of them.
final class SharedConnection {
    let handle: OpaquePointer
    init(_ handle: OpaquePointer) { self.handle = handle }
    deinit { sb_share_free(handle) }
}

/// A folder on a share: its folders to go into, and Open where it holds a notebook.
final class ShareViewController: UITableViewController {
    private struct Listing: Decodable {
        let notebook: Bool
        let folders: [String]
        let sections: [String]
    }

    private let share: SharedConnection
    private let server: Server
    private let path: String
    private var listing: Listing?
    var onOpen: ((Notebook) -> Void)?

    init(share: SharedConnection, server: Server, path: String) {
        self.share = share
        self.server = server
        self.path = path
        super.init(style: .insetGrouped)
        title = path.split(separator: "/").last.map(String.init) ?? server.share
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "entry")
        let open = UIBarButtonItem(title: "Open", primaryAction: UIAction { [weak self] _ in self?.open() })
        open.style = .done
        open.isEnabled = false
        navigationItem.rightBarButtonItem = open
        var loading = UIContentUnavailableConfiguration.loading()
        loading.text = "Loading…"
        contentUnavailableConfiguration = loading
        let (handle, path) = (Int(bitPattern: share.handle), path)
        background({
            decode(Listing.self, sb_share_list(OpaquePointer(bitPattern: handle), path))
        }) { [weak self] listing in
            guard let self else { return }
            self.listing = listing
            tableView.reloadData()
            open.isEnabled = listing.map { $0.notebook || !$0.sections.isEmpty } ?? false
            var empty = UIContentUnavailableConfiguration.empty()
            if listing == nil {
                empty.text = "Can’t Read Folder"
                empty.secondaryText = "Check that this account can open it."
            } else {
                empty.text = "No Folders"
                empty.secondaryText = "OneNote notebooks are folders with sections in them."
            }
            contentUnavailableConfiguration =
                listing.map { $0.folders.isEmpty && $0.sections.isEmpty } ?? true ? empty : nil
        }
    }

    private func open() {
        var server = server
        server.root = path
        let name = path.split(separator: "/").last.map(String.init) ?? server.share
        onOpen?(Notebook(name: name, source: .server(server)))
    }

    override func numberOfSections(in tableView: UITableView) -> Int { 2 }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        section == 0 ? listing?.folders.count ?? 0 : listing?.sections.count ?? 0
    }

    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        section == 1 && !(listing?.sections.isEmpty ?? true) ? "Sections" : nil
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "entry", for: indexPath)
        var content = cell.defaultContentConfiguration()
        if indexPath.section == 0 {
            content.text = listing?.folders[indexPath.row]
            content.image = UIImage(systemName: "folder.fill")
            cell.accessoryType = .disclosureIndicator
            cell.selectionStyle = .default
        } else {
            content.text = listing.map { URL(fileURLWithPath: $0.sections[indexPath.row]).deletingPathExtension().lastPathComponent }
            content.image = UIImage(systemName: "rectangle.portrait.fill")
            content.imageProperties.tintColor = .secondaryLabel
            content.textProperties.color = .secondaryLabel
            cell.accessoryType = .none
            cell.selectionStyle = .none
        }
        cell.contentConfiguration = content
        return cell
    }

    override func tableView(_ tableView: UITableView, willSelectRowAt indexPath: IndexPath) -> IndexPath? {
        indexPath.section == 0 ? indexPath : nil
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        guard let folder = listing?.folders[indexPath.row] else { return }
        let child = ShareViewController(
            share: share, server: server, path: path.isEmpty ? folder : "\(path)/\(folder)")
        child.onOpen = onOpen
        navigationController?.pushViewController(child, animated: true)
    }
}
