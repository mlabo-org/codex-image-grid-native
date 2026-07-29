import Foundation
import Testing
@testable import CodexImageGridNative

@Test func explicitNativeServerWinsAndBuildsTheCompleteSwiftUIProcessPlan() throws {
    let fixture = try NativeRuntimeFixture()
    defer { fixture.remove() }
    let explicit = try fixture.makeExecutable(at: fixture.root.appendingPathComponent(
        "explicit/image-grid-server"
    ))
    _ = try fixture.makeExecutable(at: fixture.bundleRoot.appendingPathComponent(
        "image-grid-server"
    ))
    _ = try fixture.makeExecutable(at: fixture.repositoryRoot.appendingPathComponent(
        "target/debug/image-grid-server"
    ))

    let plan = try NativeRuntimeResolver.resolveLaunchPlan(
        context: fixture.context(
            environment: [NativeRuntimeResolver.explicitBinaryEnvironmentKey: explicit.path],
            bundledServerURL: fixture.bundleRoot.appendingPathComponent("image-grid-server")
        )
    )

    #expect(plan.source == .explicitEnvironment)
    #expect(plan.executableURL == explicit.resolvingSymlinksInPath().standardizedFileURL)
    #expect(plan.serverRoot == fixture.repositoryRoot.resolvingSymlinksInPath().standardizedFileURL)
    #expect(plan.workspaceRoot == plan.dataRoot)
    #expect(plan.currentDirectoryURL == plan.workspaceRoot)
    #expect(plan.arguments == [
        "--bind",
        "127.0.0.1:4322",
        "--data-root",
        plan.dataRoot.path,
        "--server-root",
        plan.serverRoot.path,
        "--workspace-root",
        plan.workspaceRoot.path,
        "--launch-target",
        "swiftui",
    ])
}

@Test func bundledServerPrecedesDevelopmentBuild() throws {
    let fixture = try NativeRuntimeFixture()
    defer { fixture.remove() }
    let bundled = try fixture.makeExecutable(
        at: fixture.bundleRoot.appendingPathComponent("image-grid-server")
    )
    _ = try fixture.makeExecutable(at: fixture.repositoryRoot.appendingPathComponent(
        "target/debug/image-grid-server"
    ))

    let plan = try NativeRuntimeResolver.resolveLaunchPlan(
        context: fixture.context(bundledServerURL: bundled)
    )

    #expect(plan.source == .bundledResource)
    #expect(plan.executableURL == bundled.resolvingSymlinksInPath().standardizedFileURL)
}

@Test func developmentServerMustRemainInsideTheNativeRepository() throws {
    let fixture = try NativeRuntimeFixture()
    defer { fixture.remove() }
    let outside = try fixture.makeExecutable(
        at: fixture.root.appendingPathComponent("outside/image-grid-server")
    )
    let developmentLink = fixture.repositoryRoot.appendingPathComponent(
        "target/debug/image-grid-server"
    )
    try FileManager.default.createDirectory(
        at: developmentLink.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    try FileManager.default.createSymbolicLink(
        at: developmentLink,
        withDestinationURL: outside
    )

    do {
        _ = try NativeRuntimeResolver.resolveLaunchPlan(context: fixture.context())
        Issue.record("A development symlink escaping the native repository was accepted.")
    } catch let error as NativeRuntimeResolutionError {
        #expect(error.localizedDescription.contains("outside the allowed native runtime root"))
    }
}

@Test func explicitServerMustBeAbsoluteAndNeverFallsBack() throws {
    let fixture = try NativeRuntimeFixture()
    defer { fixture.remove() }
    let bundled = try fixture.makeExecutable(
        at: fixture.bundleRoot.appendingPathComponent("image-grid-server")
    )

    do {
        _ = try NativeRuntimeResolver.resolveLaunchPlan(
            context: fixture.context(
                environment: [
                    NativeRuntimeResolver.explicitBinaryEnvironmentKey: "image-grid-server",
                ],
                bundledServerURL: bundled
            )
        )
        Issue.record("A relative explicit server path was accepted.")
    } catch let error as NativeRuntimeResolutionError {
        #expect(error.localizedDescription.contains("must be an absolute path"))
    }
}

@Test func healthValidationRequiresNativeIdentityAndTheCanonicalServerRoot() throws {
    let fixture = try NativeRuntimeFixture()
    defer { fixture.remove() }
    let expectedRoot = fixture.repositoryRoot.resolvingSymlinksInPath().standardizedFileURL
    let valid = try JSONSerialization.data(withJSONObject: [
        "ok": true,
        "app": NativeRuntimeResolver.expectedIdentity,
        "serverRoot": expectedRoot.path,
        "packageName": NativeRuntimeResolver.expectedIdentity,
        "launchTarget": "swiftui",
        "identity": [
            "app": NativeRuntimeResolver.expectedIdentity,
            "serverRoot": expectedRoot.path,
            "packageName": NativeRuntimeResolver.expectedIdentity,
            "launchTarget": "swiftui",
        ],
    ])

    #expect(
        NativeRuntimeHealthValidation.rejection(
            data: valid,
            expectedServerRoot: expectedRoot,
            requiredLaunchTarget: "swiftui"
        ) == nil
    )

    let foreign = try JSONSerialization.data(withJSONObject: [
        "ok": true,
        "app": "codex-image-grid",
        "serverRoot": expectedRoot.path,
        "packageName": "codex-image-grid",
    ])
    #expect(
        NativeRuntimeHealthValidation.rejection(
            data: foreign,
            expectedServerRoot: expectedRoot
        )?.contains("not Codex Image Grid Native") == true
    )

    let wrongRoot = try JSONSerialization.data(withJSONObject: [
        "ok": true,
        "app": NativeRuntimeResolver.expectedIdentity,
        "serverRoot": fixture.bundleRoot.path,
        "packageName": NativeRuntimeResolver.expectedIdentity,
    ])
    #expect(
        NativeRuntimeHealthValidation.rejection(
            data: wrongRoot,
            expectedServerRoot: expectedRoot
        )?.contains("serverRoot does not match") == true
    )
}

private struct NativeRuntimeFixture {
    let root: URL
    let repositoryRoot: URL
    let bundleRoot: URL
    let applicationSupportDirectory: URL

    init() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent(
            "native-runtime-lifecycle-\(UUID().uuidString)",
            isDirectory: true
        )
        repositoryRoot = root.appendingPathComponent("codex-image-grid-native", isDirectory: true)
        bundleRoot = root.appendingPathComponent("CodexImageGridNative.app", isDirectory: true)
        applicationSupportDirectory = root.appendingPathComponent(
            "Application Support",
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: repositoryRoot.appendingPathComponent(
                "crates/image-grid-server",
                isDirectory: true
            ),
            withIntermediateDirectories: true
        )
        try Data("[workspace]\n".utf8).write(
            to: repositoryRoot.appendingPathComponent("Cargo.toml")
        )
        try FileManager.default.createDirectory(
            at: bundleRoot,
            withIntermediateDirectories: true
        )
        try FileManager.default.createDirectory(
            at: applicationSupportDirectory,
            withIntermediateDirectories: true
        )
    }

    func context(
        environment: [String: String] = [:],
        bundledServerURL: URL? = nil
    ) -> NativeRuntimeResolutionContext {
        NativeRuntimeResolutionContext(
            environment: environment,
            bundledServerURL: bundledServerURL,
            bundleRoot: bundleRoot,
            repositoryRoot: repositoryRoot,
            applicationSupportDirectory: applicationSupportDirectory,
            forbiddenRoots: [
                root.appendingPathComponent("plugin-cache", isDirectory: true),
                root.appendingPathComponent("codex-image-grid", isDirectory: true),
            ]
        )
    }

    func makeExecutable(at url: URL) throws -> URL {
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try Data("#!/bin/sh\nexit 0\n".utf8).write(to: url)
        try FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o755))],
            ofItemAtPath: url.path
        )
        return url
    }

    func remove() {
        try? FileManager.default.removeItem(at: root)
    }
}
