import XCTest
@testable import ITSaNAS

/// The Swift side of the boundary, against the real core linked into the app.
final class CoreTests: XCTestCase {
    func testAnEnvelopeErrorIsThrownWithItsSentence() {
        XCTAssertThrowsError(try Core.unwrap(#"{"error":"wrong passphrase"}"#)) { error in
            XCTAssertEqual(error.localizedDescription, "wrong passphrase")
        }
        XCTAssertEqual(try Core.unwrap(#"{"ok":"{}"}"#), "{}")
    }

    func testThePassphraseRuleComesFromTheCore() throws {
        let rule = try Core.passphraseRule()
        XCTAssertNotNil(rule["min"] as? Int, "the app would state no minimum: \(rule)")
    }

    func testNoAccountIsOpenedWithNothingRemembered() throws {
        XCTAssertFalse(try Core.openRemembered(), "a phone with nothing kept opened an account")
    }

    func testBothBackgroundGrantsAreDeclared() {
        let declared = Bundle(for: AppDelegate.self)
            .object(forInfoDictionaryKey: "BGTaskSchedulerPermittedIdentifiers") as? [String] ?? []
        XCTAssertEqual(
            Set(declared), [Background.refresh, Background.processing],
            "registering an undeclared identifier ends the app at launch")
    }

    func testABackgroundPassWithNothingRememberedDoesNothing() throws {
        Core.close()
        XCTAssertEqual(try Core.backgroundPass(metadataOnly: true)["ran"] as? Bool, false)
    }

    /// "Open in" / the share sheet: the file is copied at once (iOS's address
    /// may not outlive the call) and waits for the account; the app declares
    /// it takes any file, or no other app ever offers it.
    func testAFileSharedIntoTheAppIsCopiedAndWaitsForTheAccount() throws {
        let types = Bundle(for: AppDelegate.self)
            .object(forInfoDictionaryKey: "CFBundleDocumentTypes") as? [[String: Any]] ?? []
        let taken = types.flatMap { $0["LSItemContentTypes"] as? [String] ?? [] }
        XCTAssertTrue(taken.contains("public.item"), "no other app offers ITSaNAS a file: \(types)")

        let source = FileManager.default.temporaryDirectory.appendingPathComponent("from-another-app.txt")
        try "shared from another app".write(to: source, atomically: true, encoding: .utf8)
        Shared.waiting = []
        XCTAssertTrue(AppDelegate().application(UIApplication.shared, open: source, options: [:]))
        try FileManager.default.removeItem(at: source)
        let copy = try XCTUnwrap(Shared.waiting.first, "the shared file waits nowhere: it never reaches the account")
        XCTAssertEqual(
            try String(contentsOf: copy, encoding: .utf8), "shared from another app",
            "the copy kept is not the file shared, or did not survive the original")
        XCTAssertFalse(
            AppDelegate().application(UIApplication.shared, open: URL(string: "https://example.org")!, options: [:]),
            "a web address was taken as a file")
        Shared.waiting = []
    }

    func testNothingIsListedWithNoAccountOpen() {
        Core.close()
        XCTAssertThrowsError(try Core.listAt(""), "a closed account answered a listing")
    }
}
