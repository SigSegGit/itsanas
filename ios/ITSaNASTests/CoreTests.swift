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

    func testTrashDaysAreKeptAndNeverBelowOne() {
        let before = Core.trashDays
        defer { Core.trashDays = before }
        Core.trashDays = 7
        XCTAssertEqual(Core.trashDays, 7, "the days chosen in the trash screen were not kept")
        Core.trashDays = 0
        XCTAssertEqual(Core.trashDays, 1, "a file deleted with zero days would be gone at the next round")
    }

    func testNothingIsListedWithNoAccountOpen() {
        Core.close()
        XCTAssertThrowsError(try Core.listAt(""), "a closed account answered a listing")
    }
}
