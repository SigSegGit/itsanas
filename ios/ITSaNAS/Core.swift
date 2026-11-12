import Foundation
import LocalAuthentication

/// A fault the core reported: the sentence the command line prints for it.
struct CoreError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// The only Swift that calls the C functions. Every answer is an envelope,
/// `{"ok": "<answer>"}` or `{"error": "<sentence>"}`, freed here once.
enum Core {
    /// Where the account lives: the app's own Application Support folder,
    /// which iOS backs up with the phone unless told otherwise.
    static var home: String {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("itsanas", isDirectory: true).path
    }

    /// Unwrap an envelope into the answer text, or throw its sentence.
    static func take(_ answer: UnsafeMutablePointer<CChar>?) throws -> String {
        guard let answer = answer else { throw CoreError(message: "the core gave no answer") }
        defer { itsanas_free(answer) }
        return try unwrap(String(cString: answer))
    }

    static func unwrap(_ text: String) throws -> String {
        guard let data = text.data(using: .utf8),
              let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { throw CoreError(message: "the core wrote an unreadable answer") }
        if let error = envelope["error"] as? String { throw CoreError(message: error) }
        guard let ok = envelope["ok"] as? String else {
            throw CoreError(message: "the core wrote an unreadable answer")
        }
        return ok
    }

    /// The answer, read as a JSON object.
    static func object(_ answer: UnsafeMutablePointer<CChar>?) throws -> [String: Any] {
        let text = try take(answer)
        guard let data = text.data(using: .utf8),
              let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { throw CoreError(message: "the core wrote an unreadable answer") }
        return object
    }

    static func exists() -> Bool { itsanas_exists(home) }

    /// Debug builds only, for the UI tests: every node is pinned to their
    /// coordinator. A release build refuses it.
    static func useCoordinatorForTests(_ address: String, device: String) throws {
        _ = try take(itsanas_use_coordinator_for_tests(address, device))
    }
    static func open(passphrase: String) throws { _ = try take(itsanas_open(home, passphrase)) }
    /// Whether the Keychain's passphrase opened the account.
    static func openRemembered() throws -> Bool {
        try object(itsanas_open_remembered(home))["opened"] as? Bool ?? false
    }

    /// Whether this screen must have a finger before it opens the account.
    static var fingerNeeded: Bool { itsanas_finger_needed() }

    static func close() { _ = try? take(itsanas_close()) }
    static func listAt(_ folder: String) throws -> [String: Any] { try object(itsanas_list_at(folder)) }
    static func get(_ path: String, to destination: String) throws { _ = try take(itsanas_get(path, destination)) }
    static func put(_ path: String, from source: String) throws { _ = try take(itsanas_put(path, source)) }
    /// Let this phone's copy go once other machines prove they hold it:
    /// `freed`, and the sentence to show.
    static func release(_ path: String) throws -> [String: Any] { try object(itsanas_release(path)) }
    static func sync() throws -> [String: Any] { try object(itsanas_sync(false)) }

    /// What a round brought, named: "2 new: a, b · 1 changed: c", or nil
    /// when nothing changed. Android's `changesLine`, from the same answer.
    static func changesLine(_ answer: [String: Any]) -> String? {
        let kinds = [("new", "%d new"), ("changed", "%d changed"), ("gone", "%d removed")]
        let parts = kinds.compactMap { key, label -> String? in
            guard let part = answer[key] as? [String: Any],
                  let count = part["count"] as? Int, count > 0
            else { return nil }
            let names = (part["names"] as? [String] ?? []).map { ($0 as NSString).lastPathComponent }
            let more = count > names.count ? ", …" : ""
            return String(format: NSLocalizedString(label, comment: ""), count)
                + ": " + names.joined(separator: ", ") + more
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
    static func status() throws -> [String: Any] { try object(itsanas_status()) }
    static func backgroundPass(metadataOnly: Bool) throws -> [String: Any] {
        try object(itsanas_background_pass(home, metadataOnly))
    }

    static func passphraseRule() throws -> [String: Any] { try object(itsanas_passphrase_rule()) }

    /// Whether this iPhone has a finger or a face enrolled to unlock with.
    static var biometricAvailable: Bool {
        LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
    }

    /// Whether the fingers enrolled now are those Touch ID was turned on with.
    static func fingerMatches(_ state: String) -> Bool { itsanas_finger_matches(state) }

    /// How long a deleted file waits: 30 days unless the trash screen says
    /// otherwise, as on Android, and never less than one.
    static var trashDays: UInt64 {
        get {
            let kept = UserDefaults.standard.integer(forKey: "trashDays")
            return kept > 0 ? UInt64(kept) : 30
        }
        set { UserDefaults.standard.set(Int(min(max(newValue, 1), 9999)), forKey: "trashDays") }
    }

    static func trashPut(_ path: String) throws { _ = try take(itsanas_trash_put(path, trashDays)) }
    static func trashList() throws -> [String: Any] { try object(itsanas_trash_list()) }
    static func trashRestore(_ path: String) throws { _ = try take(itsanas_trash_restore(home, path)) }
    static func trashDelete(_ path: String) throws { _ = try take(itsanas_trash_delete(path)) }

    static func pageStart(mode: String) throws -> URL {
        try FileManager.default.createDirectory(atPath: home, withIntermediateDirectories: true)
        let text = try take(itsanas_page_start(home, mode, biometricAvailable))
        guard let url = URL(string: text) else { throw CoreError(message: "the page gave no address") }
        return url
    }

    static func pageStop() { _ = try? take(itsanas_page_stop()) }

    /// The next question or news from the page, or nil.
    static func nextRequest() -> [String: Any]? {
        guard let request = try? object(itsanas_next_request()), request["id"] != nil else { return nil }
        return request
    }

    /// Answer question `id`; nil for Back.
    static func reply(_ id: UInt64, _ text: String?) {
        if let text = text {
            _ = try? take(itsanas_reply(id, text))
        } else {
            _ = try? take(itsanas_reply(id, nil))
        }
    }
}
