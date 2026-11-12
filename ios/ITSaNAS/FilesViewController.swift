import UIKit
import UniformTypeIdentifiers

/// One folder of the account: open a subfolder, fetch a file and share or
/// save it, add a file from the Files app, run a round, open Settings.
final class FilesViewController: UITableViewController, UIDocumentPickerDelegate {
    private let folder: String
    private var folders: [[String: Any]] = []
    private var files: [[String: Any]] = []

    init(folder: String) {
        self.folder = folder
        super.init(style: .plain)
    }

    required init?(coder: NSCoder) { fatalError("not from a storyboard") }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = folder.isEmpty ? "ITSaNAS" : (folder as NSString).lastPathComponent
        navigationItem.rightBarButtonItems = [
            named(UIBarButtonItem(barButtonSystemItem: .add, target: self, action: #selector(add)), "files-add"),
            named(UIBarButtonItem(barButtonSystemItem: .refresh, target: self, action: #selector(sync)), "files-sync"),
        ]
        if folder.isEmpty {
            navigationItem.leftBarButtonItems = [
                named(UIBarButtonItem(
                    title: NSLocalizedString("Settings", comment: ""), style: .plain,
                    target: self, action: #selector(settings)), "files-settings"),
                named(UIBarButtonItem(barButtonSystemItem: .trash, target: self, action: #selector(trash)), "files-trash"),
            ]
            // The system's trash icon reads "Delete" to VoiceOver: this one
            // opens the trash, and a swiped row's Delete is another button.
            navigationItem.leftBarButtonItems?.last?.accessibilityLabel = NSLocalizedString("Trash", comment: "")
        }
        tableView.accessibilityIdentifier = "files"
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "entry")
    }

    /// What the UI tests (ios/ITSaNASUITests) tap.
    private func named(_ item: UIBarButtonItem, _ identifier: String) -> UIBarButtonItem {
        item.accessibilityIdentifier = identifier
        return item
    }

    private func reload() {
        do {
            let level = try Core.listAt(folder)
            folders = level["folders"] as? [[String: Any]] ?? []
            files = level["files"] as? [[String: Any]] ?? []
            tableView.reloadData()
        } catch {
            show(error.localizedDescription)
        }
    }

    /// Off the main thread: a round dials machines and can take a while.
    private func work(_ body: @escaping () throws -> Void, then: @escaping () -> Void = {}) {
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome = Result { try body() }
            DispatchQueue.main.async {
                if case .failure(let error) = outcome { self.show(error.localizedDescription) }
                then()
                self.reload()
            }
        }
    }

    @objc private func sync() { work { _ = try Core.sync() } }

    @objc private func settings() {
        navigationController?.pushViewController(PageViewController(mode: "settings"), animated: true)
    }

    @objc private func trash() {
        navigationController?.pushViewController(TrashViewController(), animated: true)
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        reload()
    }

    @objc private func add() {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [UTType.item], asCopy: true)
        picker.delegate = self
        present(picker, animated: true)
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        for url in urls {
            let path = folder.isEmpty ? url.lastPathComponent : folder + "/" + url.lastPathComponent
            work { try Core.put(path, from: url.path) }
        }
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        folders.count + files.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "entry", for: indexPath)
        if indexPath.row < folders.count {
            let name = folders[indexPath.row]["name"] as? String ?? ""
            cell.textLabel?.text = "📁 " + name
            cell.accessibilityIdentifier = "folder:" + name
        } else {
            let file = files[indexPath.row - folders.count]
            let name = ((file["path"] as? String ?? "") as NSString).lastPathComponent
            let here = file["here"] as? Bool ?? false
            cell.textLabel?.text = here ? name : name + " ☁︎"
            cell.accessibilityIdentifier = "file:" + name
        }
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        if indexPath.row < folders.count {
            let path = folders[indexPath.row]["path"] as? String ?? ""
            navigationController?.pushViewController(FilesViewController(folder: path), animated: true)
            return
        }
        let path = files[indexPath.row - folders.count]["path"] as? String ?? ""
        let out = FileManager.default.temporaryDirectory.appendingPathComponent((path as NSString).lastPathComponent)
        work({ try Core.get(path, to: out.path) }, then: {
            guard FileManager.default.fileExists(atPath: out.path) else { return }
            let share = UIActivityViewController(activityItems: [out], applicationActivities: nil)
            share.popoverPresentationController?.sourceView = tableView.cellForRow(at: indexPath)
            self.present(share, animated: true)
        })
    }

    override func tableView(
        _ tableView: UITableView, trailingSwipeActionsConfigurationForRowAt indexPath: IndexPath
    ) -> UISwipeActionsConfiguration? {
        guard indexPath.row >= folders.count else { return nil }
        let path = files[indexPath.row - folders.count]["path"] as? String ?? ""
        let delete = UIContextualAction(style: .destructive, title: NSLocalizedString("Delete", comment: "")) { _, _, done in
            self.work { try Core.trashPut(path) }
            done(true)
        }
        return UISwipeActionsConfiguration(actions: [delete])
    }

    private func show(_ message: String) {
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }
}
