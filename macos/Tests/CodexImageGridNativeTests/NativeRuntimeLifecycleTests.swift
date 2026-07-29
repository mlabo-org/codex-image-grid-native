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

@Test func packagedAppUsesBundledServerAndAppBundleAsServerRoot() throws {
    let fixture = try NativeRuntimeFixture()
    defer { fixture.remove() }
    let bundled = try fixture.makeExecutable(
        at: fixture.bundleRoot
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("Resources", isDirectory: true)
            .appendingPathComponent("image-grid-server")
    )
    let external = try fixture.makeExecutable(
        at: fixture.root.appendingPathComponent("external/image-grid-server")
    )

    let plan = try NativeRuntimeResolver.resolveLaunchPlan(
        context: fixture.packagedContext(
            bundledServerURL: bundled,
            environment: [
                NativeRuntimeResolver.explicitBinaryEnvironmentKey: external.path,
            ]
        )
    )

    #expect(plan.source == .bundledResource)
    #expect(plan.executableURL == bundled.resolvingSymlinksInPath().standardizedFileURL)
    #expect(plan.serverRoot == fixture.bundleRoot.resolvingSymlinksInPath().standardizedFileURL)
    #expect(plan.dataRoot.lastPathComponent == "codex-image-grid")
    #expect(plan.workspaceRoot == plan.dataRoot)
    #expect(plan.arguments.contains(plan.serverRoot.path))
    #expect(plan.arguments.suffix(2) == ["--launch-target", "swiftui"])

    let health = try JSONSerialization.data(withJSONObject: [
        "ok": true,
        "app": NativeRuntimeResolver.expectedIdentity,
        "serverRoot": plan.serverRoot.path,
        "packageName": NativeRuntimeResolver.expectedIdentity,
        "packageVersion": NativeRuntimeResolver.expectedPackageVersion,
        "packageRootKind": "packaged",
        "launchTarget": "swiftui",
    ])
    #expect(
        NativeRuntimeHealthValidation.rejection(
            data: health,
            expectedServerRoot: plan.serverRoot,
            requiredLaunchTarget: "swiftui"
        ) == nil
    )
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
        "packageVersion": NativeRuntimeResolver.expectedPackageVersion,
        "packageRootKind": "source",
        "launchTarget": "swiftui",
        "identity": [
            "app": NativeRuntimeResolver.expectedIdentity,
            "serverRoot": expectedRoot.path,
            "packageName": NativeRuntimeResolver.expectedIdentity,
            "packageVersion": NativeRuntimeResolver.expectedPackageVersion,
            "packageRootKind": "source",
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
        "app": "codex-image-grid-native",
        "serverRoot": expectedRoot.path,
        "packageName": "codex-image-grid-native",
        "packageVersion": NativeRuntimeResolver.expectedPackageVersion,
        "packageRootKind": "source",
        "launchTarget": "swiftui",
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
        "packageVersion": NativeRuntimeResolver.expectedPackageVersion,
        "packageRootKind": "source",
        "launchTarget": "swiftui",
    ])
    #expect(
        NativeRuntimeHealthValidation.rejection(
            data: wrongRoot,
            expectedServerRoot: expectedRoot
        )?.contains("serverRoot does not match") == true
    )
}

@Test @MainActor
func joinedRuntimeShutdownNeverSignalsTheUnownedProcess() async {
    let process = NativeRuntimeProcessDouble(
        processIdentifier: 41,
        exitsOnTerminate: false
    )
    let lifecycle = NativeRuntimeLifecycle(
        shutdownGracePeriod: .milliseconds(5),
        initialProcess: process,
        initialOwnership: .joined
    )

    await lifecycle.stop()

    let snapshot = await process.snapshot()
    #expect(snapshot.terminateCount == 0)
    #expect(snapshot.forceKillProcessIdentifiers.isEmpty)
    #expect(lifecycle.state == .idle)
}

@Test @MainActor
func ownedRuntimeShutdownSendsOneTerminateAndWaitsForExit() async {
    let process = NativeRuntimeProcessDouble(
        processIdentifier: 42,
        exitsOnTerminate: true
    )
    let lifecycle = NativeRuntimeLifecycle(
        initialProcess: process,
        initialOwnership: .launched
    )

    await lifecycle.stop()

    let snapshot = await process.snapshot()
    #expect(snapshot.terminateCount == 1)
    #expect(snapshot.waitCount == 1)
    #expect(snapshot.forceKillProcessIdentifiers.isEmpty)
    #expect(!snapshot.isRunning)
    #expect(lifecycle.state == .idle)
}

@Test @MainActor
func concurrentAndRepeatedShutdownCallersShareOneCompletion() async {
    let process = NativeRuntimeProcessDouble(
        processIdentifier: 43,
        exitsOnTerminate: false
    )
    let lifecycle = NativeRuntimeLifecycle(
        shutdownGracePeriod: .seconds(1),
        initialProcess: process,
        initialOwnership: .launched
    )

    let first = Task { @MainActor in
        await lifecycle.stop()
    }
    let second = Task { @MainActor in
        await lifecycle.stop()
    }
    await waitUntilTerminateWasSent(to: process)
    await process.exitNormally()
    await first.value
    await second.value
    await lifecycle.stop()

    let snapshot = await process.snapshot()
    #expect(snapshot.terminateCount == 1)
    #expect(snapshot.forceKillProcessIdentifiers.isEmpty)
    #expect(!snapshot.isRunning)
    #expect(lifecycle.state == .idle)
}

@Test @MainActor
func ownedRuntimeUsesBoundedExactPidForceKillFallback() async {
    let processIdentifier: Int32 = 44
    let process = NativeRuntimeProcessDouble(
        processIdentifier: processIdentifier,
        exitsOnTerminate: false
    )
    let lifecycle = NativeRuntimeLifecycle(
        shutdownGracePeriod: .milliseconds(5),
        initialProcess: process,
        initialOwnership: .launched
    )

    await lifecycle.stop()

    let snapshot = await process.snapshot()
    #expect(NativeRuntimeLifecycle.shutdownGracePeriod == .seconds(6))
    #expect(snapshot.terminateCount == 1)
    #expect(snapshot.forceKillProcessIdentifiers == [processIdentifier])
    #expect(!snapshot.isRunning)
    #expect(lifecycle.state == .idle)
}

private struct NativeRuntimeProcessSnapshot: Sendable {
    let isRunning: Bool
    let terminateCount: Int
    let waitCount: Int
    let forceKillProcessIdentifiers: [Int32]
}

private actor NativeRuntimeProcessDouble: NativeRuntimeProcessHandle {
    let processIdentifier: Int32
    private let exitsOnTerminate: Bool
    private var running = true
    private var terminateCount = 0
    private var waitCount = 0
    private var forceKillProcessIdentifiers: [Int32] = []

    init(processIdentifier: Int32, exitsOnTerminate: Bool) {
        self.processIdentifier = processIdentifier
        self.exitsOnTerminate = exitsOnTerminate
    }

    func isRunning() -> Bool {
        running
    }

    func terminationStatus() -> Int32 {
        running ? 0 : 15
    }

    func sendTerminate() {
        terminateCount += 1
        if exitsOnTerminate {
            running = false
        }
    }

    func waitUntilExit() async {
        waitCount += 1
        while running {
            guard !Task.isCancelled else { return }
            do {
                try await Task.sleep(for: .milliseconds(1))
            } catch {
                return
            }
        }
    }

    func forceKill(expectedProcessIdentifier: Int32) {
        forceKillProcessIdentifiers.append(expectedProcessIdentifier)
        if expectedProcessIdentifier == processIdentifier {
            running = false
        }
    }

    func exitNormally() {
        running = false
    }

    func snapshot() -> NativeRuntimeProcessSnapshot {
        NativeRuntimeProcessSnapshot(
            isRunning: running,
            terminateCount: terminateCount,
            waitCount: waitCount,
            forceKillProcessIdentifiers: forceKillProcessIdentifiers
        )
    }
}

private func waitUntilTerminateWasSent(
    to process: NativeRuntimeProcessDouble
) async {
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: .seconds(1))
    while clock.now < deadline {
        if await process.snapshot().terminateCount == 1 {
            return
        }
        try? await Task.sleep(for: .milliseconds(1))
    }
    Issue.record("The lifecycle did not send SIGTERM to its owned process.")
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

    func packagedContext(
        bundledServerURL: URL,
        environment: [String: String] = [:]
    ) -> NativeRuntimeResolutionContext {
        NativeRuntimeResolutionContext(
            environment: environment,
            bundledServerURL: bundledServerURL,
            bundleRoot: bundleRoot,
            repositoryRoot: nil,
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
