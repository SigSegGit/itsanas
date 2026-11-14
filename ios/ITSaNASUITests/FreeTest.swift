import XCTest

/// Free up on the iPhone never lets go of a copy no other machine has proved
/// it holds. The account `scripts/iphone-e2e.sh` makes has one computer, and a
/// file goes only once two machines have answered a storage challenge for it,
/// so the phone must keep it and say why. Android's FreeTest, on the iPhone.
final class FreeTest: E2eCase {
    func testFreeUpKeepsAFileNoOtherMachineHasProvedItHolds() throws {
        try logIn()
        try syncUntilListed("from-computer.txt")
        let row = app.cells["file:from-computer.txt"]
        if row.label.contains("☁︎") {
            row.tap()
            let shared = app.otherElements["ActivityListView"]
            try waitFor("a file from the computer did not reach the phone", seconds: 60) {
                shared.exists || app.alerts.count > 0
            }
            if app.alerts.count > 0 { try fail("fetching a file from the computer was refused") }
            let close = shared.buttons["Close"]
            if close.exists { close.tap() } else { shared.swipeDown() }
        }
        try waitFor("the fetched file is still shown as not on this phone") { !row.label.contains("☁︎") }

        row.swipeLeft()
        let free = app.buttons["Free up"]
        try waitFor("a file kept on the phone offers no Free up") { free.exists }
        free.tap()
        let told = app.staticTexts["files-told"]
        try waitFor("Free up said nothing, or let the file go", seconds: 60) {
            told.exists && told.label.hasPrefix("Kept here")
        }
        XCTAssertTrue(
            row.exists && !row.label.contains("☁︎"),
            "the iPhone let go of a file no other machine holds: it is lost")
    }
}
