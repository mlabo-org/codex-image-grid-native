import Foundation
import Testing
@testable import CodexImageGridNative

@Test func theAPIKeyRoundTripsThroughTheKeychain() throws {
    // A service of its own, so the test never touches the key the app saved.
    let store = OpenAIAPIKeyStore(service: "local.codex.image-grid.native.tests.\(UUID().uuidString)")
    defer { try? store.delete() }

    #expect(store.read() == nil)

    try store.save("sk-test-first")
    try store.save("sk-test-second")
    #expect(store.read() == "sk-test-second")

    try store.delete()
    try store.delete()
    #expect(store.read() == nil)
}
