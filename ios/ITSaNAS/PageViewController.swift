import UIKit
import WebKit

/// The shared page (account creation, then Settings), served by the core on
/// 127.0.0.1 and shown in a web view, as the Android app does. Its secrets
/// are never typed in the page: the engine asks for them, and this screen
/// answers from a native alert.
final class PageViewController: UIViewController {
    private let mode: String
    private let web = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
    private var poll: Timer?
    private var asking = false

    init(mode: String) {
        self.mode = mode
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("not from a storyboard") }

    override func loadView() { view = web }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "ITSaNAS"
        do {
            web.load(URLRequest(url: try Core.pageStart(mode: mode)))
        } catch {
            show(error.localizedDescription)
        }
        poll = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in self?.look() }
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if isMovingFromParent {
            poll?.invalidate()
            Core.pageStop()
        }
    }

    /// One question at a time: the engine waits for each answer.
    private func look() {
        guard !asking, let request = Core.nextRequest() else { return }
        let id = (request["id"] as? NSNumber)?.uint64Value ?? 0
        let kind = request["kind"] as? String ?? ""
        let args = request["args"] as? [String] ?? []
        switch kind {
        case "event": event(args)
        case "askNewPassphrase": ask(id, NSLocalizedString("Choose a passphrase", comment: ""), nil, secure: true)
        case "askPassphrase": ask(id, NSLocalizedString("Passphrase", comment: ""), args.first, secure: true)
        case "askRecoveryPhrase": ask(id, NSLocalizedString("Your 24 words", comment: ""), nil, secure: false)
        case "showWords": tell(id, NSLocalizedString("Write these 24 words down", comment: ""), args.first ?? "")
        case "showAndConfirm":
            let at = args.dropFirst().joined(separator: ", ")
            let format = NSLocalizedString("Type words %@, in order", comment: "")
            ask(id, String(format: format, at), nil, secure: false)
        default:
            Core.reply(id, nil)
        }
    }

    private func event(_ args: [String]) {
        guard args.first == "closed" else { return }
        poll?.invalidate()
        Core.pageStop()
        navigationController?.setViewControllers([AppDelegate.first()], animated: true)
    }

    private func ask(_ id: UInt64, _ title: String, _ detail: String?, secure: Bool) {
        asking = true
        let alert = UIAlertController(title: title, message: detail, preferredStyle: .alert)
        alert.addTextField { field in
            field.isSecureTextEntry = secure
            field.autocapitalizationType = .none
            field.autocorrectionType = .no
        }
        alert.addAction(UIAlertAction(title: NSLocalizedString("Back", comment: ""), style: .cancel) { _ in
            Core.reply(id, nil)
            self.asking = false
        })
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
            Core.reply(id, alert.textFields?.first?.text ?? "")
            self.asking = false
        })
        present(alert, animated: true)
    }

    private func tell(_ id: UInt64, _ title: String, _ text: String) {
        asking = true
        let alert = UIAlertController(title: title, message: text, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
            Core.reply(id, "")
            self.asking = false
        })
        present(alert, animated: true)
    }

    private func show(_ message: String) {
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }
}
