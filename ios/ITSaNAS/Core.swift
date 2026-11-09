import Foundation

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
    static func open(passphrase: String) throws { _ = try take(itsanas_open(home, passphrase)) }
    /// Whether the Keychain's passphrase opened the account.
    static func openRemembered() throws -> Bool {
        try object(itsanas_open_remembered(home))["opened"] as? Bool ?? false
    }

    static func close() { _ = try? take(itsanas_close()) }
    static func listAt(_ folder: String) throws -> [String: Any] { try object(itsanas_list_at(folder)) }
    static func get(_ path: String, to destination: String) throws { _ = try take(itsanas_get(path, destination)) }
    static func put(_ path: String, from source: String) throws { _ = try take(itsanas_put(path, source)) }
    static func sync() throws -> [String: Any] { try object(itsanas_sync(false)) }
    static func status() throws -> [String: Any] { try object(itsanas_status()) }
    static func passphraseRule() throws -> [String: Any] { try object(itsanas_passphrase_rule()) }

    static func pageStart(mode: String) throws -> URL {
        try FileManager.default.createDirectory(atPath: home, withIntermediateDirectories: true)
        let text = try take(itsanas_page_start(home, mode, false))
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
