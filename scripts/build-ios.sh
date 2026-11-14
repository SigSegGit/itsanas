#!/bin/sh
# Build the iPhone app on a Mac: the Rust core for the phone and for the
# simulator, then the Xcode project from ios/project.yml.
#
#   sh scripts/build-ios.sh          core and project; open ios/ITSaNAS.xcodeproj
#   sh scripts/build-ios.sh test     and run the Swift tests on a simulator: the
#                                    unit tests, then the hand test's UI tests
#                                    (scripts/iphone-e2e.sh)
#   sh scripts/build-ios.sh ipa      and package target/ITSaNAS-unsigned.ipa
#   sh scripts/build-ios.sh ci       test, then ipa
#
# Needs Xcode, XcodeGen (`brew install xcodegen`) and
# `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`.
# The core is built before the project, never by it: an app linked against a
# stale libitsanas_ios.a runs yesterday's core with no error anywhere.
set -eu
cd "$(dirname "$0")/.."

# The first iPhone SE stops at iOS 15; 14 is the floor this app keeps.
export IPHONEOS_DEPLOYMENT_TARGET=14.0
export IPHONESIMULATOR_DEPLOYMENT_TARGET=14.0

cargo build --release --locked -p itsanas-ios --target aarch64-apple-ios
cargo build --release --locked -p itsanas-ios --target aarch64-apple-ios-sim
# The simulator's Debug build links this one (ios/project.yml): only a debug
# core takes the UI tests' coordinator.
cargo build --locked -p itsanas-ios --target aarch64-apple-ios-sim
(cd ios && xcodegen generate)

what="${1:-}"

if [ "$what" = test ] || [ "$what" = ci ]; then
    # An iPhone SE first: Touch ID, as the hand test's phone, and iOS's floor.
    # Chosen by its id, on the runtime of this Xcode's SDK: a name such as
    # "iPhone SE (3rd generation)" is on several runtimes, and cut at its
    # parenthesis it named no device at all (run 42961293278).
    sdk=$(xcrun --sdk iphonesimulator --show-sdk-version)
    phones=$(xcrun simctl list devices available |
        awk -v runtime="-- iOS $sdk --" '/^-- /{ on = ($0 == runtime) } on && /^ +iPhone /')
    phone=$(printf '%s\n' "$phones" | grep 'iPhone SE' | head -n 1 || true)
    [ -n "$phone" ] || phone=$(printf '%s\n' "$phones" | head -n 1)
    simulator=$(printf '%s\n' "$phone" | grep -oE '[0-9A-F]{8}-([0-9A-F]{4}-){3}[0-9A-F]{12}' | head -n 1)
    [ -n "$simulator" ] || { echo "no iPhone simulator is installed for iOS $sdk" >&2; exit 1; }
    echo "simulator: $phone"
    xcodebuild -project ios/ITSaNAS.xcodeproj -scheme ITSaNAS \
        -destination "platform=iOS Simulator,id=$simulator" \
        -derivedDataPath target/ios-test -only-testing:ITSaNASTests test
    bash scripts/iphone-e2e.sh "$simulator"
fi

if [ "$what" = ipa ] || [ "$what" = ci ]; then
    # Unsigned: whoever installs it signs it with their own Apple ID, on their
    # Mac or in the tool that installs it.
    xcodebuild -project ios/ITSaNAS.xcodeproj -scheme ITSaNAS -configuration Release \
        -sdk iphoneos -derivedDataPath target/ios-build CODE_SIGNING_ALLOWED=NO build
    rm -rf target/ios-ipa target/ITSaNAS-unsigned.ipa
    mkdir -p target/ios-ipa/Payload
    cp -R target/ios-build/Build/Products/Release-iphoneos/ITSaNAS.app target/ios-ipa/Payload/
    (cd target/ios-ipa && zip -qr ../ITSaNAS-unsigned.ipa Payload)
    echo "target/ITSaNAS-unsigned.ipa"
fi
