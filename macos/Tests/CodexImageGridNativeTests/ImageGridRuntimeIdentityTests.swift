import Foundation
import Testing
@testable import CodexImageGridNative

@Test func validNativeHealthIdentityDecodesAndValidates() throws {
    let response = try decodeHealth([
        "ok": true,
        "generatedDir": "/tmp/codex-image-grid-native/generated",
        "identity": [
            "app": "codex-image-grid",
            "packageName": "codex-image-grid",
            "packageVersion": "0.2.0",
            "packageRootKind": "packaged",
            "launchTarget": "swiftui",
            "serverRoot": "/Users/example/Applications/Codex Image Grid Native.app",
        ],
    ])

    let identity = try ImageGridRuntimeIdentityValidation.validate(response)

    #expect(identity.app == "codex-image-grid")
    #expect(identity.packageName == "codex-image-grid")
    #expect(identity.packageVersion == "0.2.0")
    #expect(identity.packageRootKind == "packaged")
    #expect(identity.launchTarget == "swiftui")
    #expect(identity.serverRoot == "/Users/example/Applications/Codex Image Grid Native.app")
}

@Test func healthIdentityRequiresDeclarationAndAbsoluteServerRoot() throws {
    let undeclared = try decodeHealth(["ok": true])
    #expect(throws: ImageGridAPIError.self) {
        try ImageGridRuntimeIdentityValidation.validate(undeclared)
    }

    let missingRoot = try decodeHealth([
        "ok": true,
        "app": "codex-image-grid",
        "packageName": "codex-image-grid",
        "packageVersion": "0.2.0",
        "packageRootKind": "packaged",
        "launchTarget": "swiftui",
    ])
    do {
        _ = try ImageGridRuntimeIdentityValidation.validate(missingRoot)
        Issue.record("A native identity without serverRoot was accepted.")
    } catch let error as ImageGridAPIError {
        #expect(error.message.contains("nonempty absolute serverRoot"))
    }

    let relativeRoot = try decodeHealth([
        "ok": true,
        "app": "codex-image-grid",
        "packageName": "codex-image-grid",
        "packageVersion": "0.2.0",
        "packageRootKind": "packaged",
        "launchTarget": "swiftui",
        "serverRoot": "codex-image-grid-native",
    ])
    #expect(throws: ImageGridAPIError.self) {
        try ImageGridRuntimeIdentityValidation.validate(relativeRoot)
    }
}

@Test func frozenElectronHealthWithThePublicNameIsStillRejected() throws {
    let response = try decodeHealth([
        "ok": true,
        "app": "codex-image-grid",
        "packageName": "codex-image-grid",
        "packageVersion": "0.1.0",
        "packageRootKind": "packaged",
        "launchTarget": "electron",
        "serverRoot": "/Users/example/codex-image-grid",
    ])

    do {
        _ = try ImageGridRuntimeIdentityValidation.validate(response)
        Issue.record("The frozen Electron listener was accepted as the native runtime.")
    } catch let error as ImageGridAPIError {
        #expect(error.message.contains("packageVersion"))
    }
}

@Test func headlessRuntimeWithCurrentPublicMetadataIsRejected() throws {
    let response = try decodeHealth([
        "ok": true,
        "app": "codex-image-grid",
        "packageName": "codex-image-grid",
        "packageVersion": "0.2.0",
        "packageRootKind": "packaged",
        "launchTarget": "mcp",
        "serverRoot": "/Users/example/Applications/Codex Image Grid Native.app",
    ])

    do {
        _ = try ImageGridRuntimeIdentityValidation.validate(response)
        Issue.record("A headless runtime was accepted as the SwiftUI-owned runtime.")
    } catch let error as ImageGridAPIError {
        #expect(error.message.contains("launchTarget=swiftui"))
    }
}

private func decodeHealth(_ object: [String: Any]) throws -> ImageGridHealthResponse {
    let data = try JSONSerialization.data(withJSONObject: object)
    return try JSONDecoder().decode(ImageGridHealthResponse.self, from: data)
}
