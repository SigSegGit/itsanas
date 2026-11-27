import XCTest
@testable import ITSaNAS

/// The Swift side of the boundary, against the real core linked into the app.
final class CoreTests: XCTestCase {
    /// An app with no launch screen runs, on any iPhone newer than the first
    /// SE, in a box the first SE's size (320 points wide) with black bars
    /// around it, because iOS cannot know it draws at any other size. The
    /// first SE is exactly that size, so the hand test's phone never showed
    /// it. Drawn at the screen's native width, `bounds` times `nativeScale`
    /// is the panel's width in pixels; in the box it falls short. Run on the
    /// SE and on the newest iPhone the simulator has (scripts/build-ios.sh).
    func testTheAppFillsTheScreenOfAnyIPhoneRatherThanTheFirstSEsBox() {
        let screen = UIScreen.main
        let drawn = min(screen.bounds.width, screen.bounds.height) * screen.nativeScale
        let panel = min(screen.nativeBounds.width, screen.nativeBounds.height)
        XCTAssertEqual(
            drawn, panel, accuracy: 2,
            "the app draws \(screen.bounds.size) points on a \(screen.nativeBounds.size)-pixel panel: it runs in the compatibility box, with black bars, on this iPhone")
    }

    func testAnEnvelopeErrorIsThrownWithItsSentence() {
        XCTAssertThrowsError(try Core.unwrap(#"{"error":"wrong passphrase"}"#)) { error in
            XCTAssertEqual(error.localizedDescription, "wrong passphrase")
        }
        XCTAssertEqual(try Core.unwrap(#"{"ok":"{}"}"#), "{}")
    }

    func testARoundNamesWhatItBroughtAndSaysNothingWhenNothingChanged() {
        let none: [String: Any] = ["count": 0, "names": [String]()]
        XCTAssertNil(
            Core.changesLine(["new": none, "changed": none, "gone": none, "reached": 1]),
            "a round that changed nothing claimed it did")
        let said = Core.changesLine([
            "new": ["count": 7, "names": ["photos/a.jpg", "b.txt"]],
            "changed": ["count": 1, "names": ["notes/c.txt"]],
            "gone": none,
        ])
        XCTAssertEqual(said, "7 new: a.jpg, b.txt, … · 1 changed: c.txt")
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

    /// Creating an account on the iPhone asked for words 3, 11 and 19 and
    /// showed the 24 words nowhere.
    func testAccountCreationShowsTheWordsBeforeAskingForThemBack() {
        let phrase = "abandon ability able about above absent absorb abstract"
        let steps = PageViewController.confirming([phrase, "3", "11", "19"])
        XCTAssertEqual(steps.words, phrase, "the words to write down are not shown")
        XCTAssertTrue(steps.question.contains("3, 11, 19"), "the question names other words: \(steps.question)")
    }

    func testNothingIsListedWithNoAccountOpen() {
        Core.close()
        XCTAssertThrowsError(try Core.listAt(""), "a closed account answered a listing")
    }
}
