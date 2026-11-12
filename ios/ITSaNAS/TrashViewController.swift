import LocalAuthentication
import UIKit

/// What was deleted and still waits: put it back, or delete it for good. What a
/// delete does, and for how many days, is this phone's rules, set in Settings,
/// Trash (HANDOVER §8 13); a delete for good asks who is there first when they
/// say so, once for the whole trash.
final class TrashViewController: UITableViewController {
    private var rows: [[String: Any]] = []
    private var now: Double = 0
    private var askAuth = true
    private var confirm = true

    override func viewDidLoad() {
        super.viewDidLoad()
        title = NSLocalizedString("Trash", comment: "")
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "entry")
        let empty = UIBarButtonItem(
            title: NSLocalizedString("Empty", comment: ""), style: .plain, target: self, action: #selector(emptyAll))
        empty.accessibilityIdentifier = "trash-empty"
        navigationItem.rightBarButtonItem = empty
        reload()
    }

    @objc private func emptyAll() {
        let files = rows.compactMap { $0["path"] as? String }
        guard !files.isEmpty else { return }
        deleteForGood(files)
    }

    private func reload() {
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome = Result { try Core.trashList() }
            DispatchQueue.main.async {
                switch outcome {
                case .success(let list):
                    self.rows = list["trashed"] as? [[String: Any]] ?? []
                    self.now = (list["now"] as? NSNumber)?.doubleValue ?? 0
                    self.askAuth = list["ask_auth"] as? Bool ?? true
                    self.confirm = list["confirm"] as? Bool ?? true
                    self.tableView.reloadData()
                case .failure(let error):
                    self.show(error.localizedDescription)
                }
            }
        }
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        rows.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "entry", for: indexPath)
        let due = (rows[indexPath.row]["due"] as? NSNumber)?.doubleValue ?? 0
        let days = max(0, Int(((due - now) / 86_400).rounded(.up)))
        let format = NSLocalizedString("%@ (%d days)", comment: "")
        cell.textLabel?.text = String(format: format, path(indexPath), days)
        cell.accessibilityIdentifier = "trash:" + path(indexPath)
        return cell
    }

    private func path(_ indexPath: IndexPath) -> String {
        rows[indexPath.row]["path"] as? String ?? ""
    }

    override func tableView(
        _ tableView: UITableView, trailingSwipeActionsConfigurationForRowAt indexPath: IndexPath
    ) -> UISwipeActionsConfiguration? {
        let file = path(indexPath)
        let restore = UIContextualAction(style: .normal, title: NSLocalizedString("Restore", comment: "")) { _, _, done in
            self.act { try Core.trashRestore(file) }
            done(true)
        }
        let delete = UIContextualAction(style: .destructive, title: NSLocalizedString("Delete", comment: "")) { _, _, done in
            self.deleteForGood([file])
            done(false)
        }
        return UISwipeActionsConfiguration(actions: [delete, restore])
    }

    /// "Are you sure" when the rules ask it, then who is there when they ask
    /// that, then every file of `files`, gone from every machine.
    private func deleteForGood(_ files: [String]) {
        let go = {
            self.proveIdentity {
                self.act { for file in files { try Core.trashDelete(file) } }
            }
        }
        guard confirm else { return go() }
        let alert = UIAlertController(
            title: NSLocalizedString("Delete from every machine?", comment: ""),
            message: files.count == 1 ? files[0] : String(format: NSLocalizedString("%d files", comment: ""), files.count),
            preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: NSLocalizedString("Back", comment: ""), style: .cancel))
        alert.addAction(UIAlertAction(title: NSLocalizedString("Delete", comment: ""), style: .destructive) { _ in go() })
        present(alert, animated: true)
    }

    /// The phone's own lock (Face ID, Touch ID, passcode) when it has one, the
    /// account's passphrase otherwise; `proved` runs only when it answered.
    private func proveIdentity(_ proved: @escaping () -> Void) {
        guard askAuth else { return proved() }
        let context = LAContext()
        let reason = NSLocalizedString("Delete files for good", comment: "")
        if context.canEvaluatePolicy(.deviceOwnerAuthentication, error: nil) {
            context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason) { success, _ in
                DispatchQueue.main.async {
                    if success { proved() } else { self.show(NSLocalizedString("Not confirmed: nothing was deleted.", comment: "")) }
                }
            }
            return
        }
        let alert = UIAlertController(
            title: NSLocalizedString("Who is there?", comment: ""), message: reason, preferredStyle: .alert)
        alert.addTextField { field in
            field.isSecureTextEntry = true
            field.accessibilityIdentifier = "identity-pass"
        }
        alert.addAction(UIAlertAction(title: NSLocalizedString("Back", comment: ""), style: .cancel))
        alert.addAction(UIAlertAction(title: NSLocalizedString("Confirm", comment: ""), style: .default) { _ in
            let typed = alert.textFields?.first?.text ?? ""
            DispatchQueue.global(qos: .userInitiated).async {
                let right = (try? Core.open(passphrase: typed)) != nil
                DispatchQueue.main.async {
                    if right { proved() } else { self.show(NSLocalizedString("Not confirmed: nothing was deleted.", comment: "")) }
                }
            }
        })
        present(alert, animated: true)
    }

    private func act(_ body: @escaping () throws -> Void) {
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome = Result { try body() }
            DispatchQueue.main.async {
                if case .failure(let error) = outcome { self.show(error.localizedDescription) }
                self.reload()
            }
        }
    }

    private func show(_ message: String) {
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }
}
