import Foundation
import Testing
@testable import CodexImageGridNative

@Test func validNativeHealthIdentityDecodesAndValidates() throws {
    let response = try decodeHealth([
        "ok": true,
        "generatedDir": "/tmp/codex-image-grid-native/generated",
        "identity": [
            "app": "codex-image-grid-native",
            "packageName": "codex-image-grid-native",
            "serverRoot": "/Users/example/codex-image-grid-native",
        ],
    ])

    let identity = try ImageGridRuntimeIdentityValidation.validate(response)

    #expect(identity.app == "codex-image-grid-native")
    #expect(identity.packageName == "codex-image-grid-native")
    #expect(identity.serverRoot == "/Users/example/codex-image-grid-native")
}

@Test func healthIdentityRequiresDeclarationAndAbsoluteServerRoot() throws {
    let undeclared = try decodeHealth(["ok": true])
    #expect(throws: ImageGridAPIError.self) {
        try ImageGridRuntimeIdentityValidation.validate(undeclared)
    }

    let missingRoot = try decodeHealth([
        "ok": true,
        "app": "codex-image-grid-native",
        "packageName": "codex-image-grid-native",
    ])
    do {
        _ = try ImageGridRuntimeIdentityValidation.validate(missingRoot)
        Issue.record("A native identity without serverRoot was accepted.")
    } catch let error as ImageGridAPIError {
        #expect(error.message.contains("nonempty absolute serverRoot"))
    }

    let relativeRoot = try decodeHealth([
        "ok": true,
        "app": "codex-image-grid-native",
        "packageName": "codex-image-grid-native",
        "serverRoot": "codex-image-grid-native",
    ])
    #expect(throws: ImageGridAPIError.self) {
        try ImageGridRuntimeIdentityValidation.validate(relativeRoot)
    }
}

@Test func frozenElectronHealthIdentityIsRejected() throws {
    let response = try decodeHealth([
        "ok": true,
        "app": "codex-image-grid",
        "packageName": "codex-image-grid",
        "serverRoot": "/Users/example/codex-image-grid",
    ])

    do {
        _ = try ImageGridRuntimeIdentityValidation.validate(response)
        Issue.record("The frozen Electron listener was accepted as the native runtime.")
    } catch let error as ImageGridAPIError {
        #expect(error.message.contains("not Codex Image Grid Native"))
        #expect(error.message.contains("codex-image-grid-native"))
    }
}

private func decodeHealth(_ object: [String: Any]) throws -> ImageGridHealthResponse {
    let data = try JSONSerialization.data(withJSONObject: object)
    return try JSONDecoder().decode(ImageGridHealthResponse.self, from: data)
}
