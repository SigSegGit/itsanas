import UIKit

/// Asks the passphrase and opens the account.
final class UnlockViewController: UIViewController, UITextFieldDelegate {
    private let field = UITextField()
    private let message = UILabel()
    private var trying = false

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "ITSaNAS"
        view.backgroundColor = .systemBackground
        field.placeholder = NSLocalizedString("Passphrase", comment: "")
        field.isSecureTextEntry = true
        field.borderStyle = .roundedRect
        field.returnKeyType = .go
        field.delegate = self
        message.numberOfLines = 0
        message.textColor = .systemRed
        let button = UIButton(type: .system)
        button.setTitle(NSLocalizedString("Unlock", comment: ""), for: .normal)
        button.addTarget(self, action: #selector(unlock), for: .touchUpInside)
        let stack = UIStackView(arrangedSubviews: [field, button, message])
        stack.axis = .vertical
        stack.spacing = 16
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.layoutMarginsGuide.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: view.layoutMarginsGuide.trailingAnchor),
            stack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 32),
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        NotificationCenter.default.addObserver(
            self, selector: #selector(tryRemembered),
            name: UIApplication.didBecomeActiveNotification, object: nil)
        tryRemembered()
    }

    /// The Keychain's passphrase, when there is one, so that coming back to
    /// the app does not ask for it. A stale one only leaves this screen up.
    @objc private func tryRemembered() {
        guard !trying, view.window != nil, UIApplication.shared.applicationState == .active else { return }
        trying = true
        DispatchQueue.global(qos: .userInitiated).async {
            let opened = (try? Core.openRemembered()) ?? false
            DispatchQueue.main.async {
                self.trying = false
                if opened { self.showFiles() }
            }
        }
    }

    private func showFiles() {
        navigationController?.setViewControllers([FilesViewController(folder: "")], animated: true)
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        unlock()
        return true
    }

    @objc private func unlock() {
        let passphrase = field.text ?? ""
        message.text = nil
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome = Result { try Core.open(passphrase: passphrase) }
            DispatchQueue.main.async {
                switch outcome {
                case .success:
                    self.field.text = nil
                    self.showFiles()
                case .failure(let error):
                    self.message.text = error.localizedDescription
                }
            }
        }
    }
}
