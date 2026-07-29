import Combine
import Foundation

enum NativeRuntimeOwnership: String, Equatable, Sendable {
    case joined
    case launched
}

enum NativeRuntimeLifecycleState: Equatable, Sendable {
    case idle
    case checking
    case launching
    case ready(NativeRuntimeOwnership)
    case failed(String)
}

enum NativeRuntimeServerSource: String, Equatable, Sendable {
    case explicitEnvironment
    case bundledResource
    case developmentBuild
}

struct NativeRuntimeLaunchPlan: Equatable, Sendable {
    let source: NativeRuntimeServerSource
    let executableURL: URL
    let arguments: [String]
    let currentDirectoryURL: URL
    let dataRoot: URL
    let serverRoot: URL
    let workspaceRoot: URL
}

struct NativeRuntimeResolutionContext: Sendable {
    let environment: [String: String]
    let bundledServerURL: URL?
    let bundleRoot: URL
    let repositoryRoot: URL?
    let applicationSupportDirectory: URL
    let forbiddenRoots: [URL]
}

enum NativeRuntimeResolutionError: LocalizedError, Equatable {
    case invalidConfiguration(String)

    var errorDescription: String? {
        switch self {
        case let .invalidConfiguration(message):
            message
        }
    }
}

enum NativeRuntimeResolver {
    static let expectedIdentity = "codex-image-grid-native"
    static let bindAddress = "127.0.0.1:4322"
    static let executableName = "image-grid-server"
    static let explicitBinaryEnvironmentKey = "IMAGE_GRID_NATIVE_SERVER_BIN"

    static func productionContext() throws -> NativeRuntimeResolutionContext {
        let fileManager = FileManager.default
        guard
            let applicationSupportDirectory = fileManager.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            ).first
        else {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "The native Application Support directory is unavailable."
            )
        }

        let bundle = Bundle.main
        let bundledServerURL =
            bundle.url(forResource: executableName, withExtension: nil)
            ?? bundle.resourceURL
                .map { $0.appendingPathComponent(executableName, isDirectory: false) }
                .flatMap { fileManager.fileExists(atPath: $0.path) ? $0 : nil }
        let repositoryRoot = developmentRepositoryRoot(sourceFilePath: #filePath)
        let homeDirectory = fileManager.homeDirectoryForCurrentUser
        var forbiddenRoots = [
            homeDirectory
                .appendingPathComponent(".codex", isDirectory: true)
                .appendingPathComponent("plugins", isDirectory: true)
                .appendingPathComponent("cache", isDirectory: true),
        ]
        if let repositoryRoot {
            forbiddenRoots.append(
                repositoryRoot
                    .deletingLastPathComponent()
                    .appendingPathComponent("codex-image-grid", isDirectory: true)
            )
        }

        return NativeRuntimeResolutionContext(
            environment: ProcessInfo.processInfo.environment,
            bundledServerURL: bundledServerURL,
            bundleRoot: bundle.bundleURL,
            repositoryRoot: repositoryRoot,
            applicationSupportDirectory: applicationSupportDirectory,
            forbiddenRoots: forbiddenRoots
        )
    }

    static func resolveLaunchPlan(
        context: NativeRuntimeResolutionContext
    ) throws -> NativeRuntimeLaunchPlan {
        let paths = try prepareRuntimePaths(context: context)
        let resolvedExecutable: (NativeRuntimeServerSource, URL)

        if let explicitValue = context.environment[explicitBinaryEnvironmentKey]?
            .trimmingCharacters(in: .whitespacesAndNewlines),
            !explicitValue.isEmpty
        {
            guard NSString(string: explicitValue).isAbsolutePath else {
                throw NativeRuntimeResolutionError.invalidConfiguration(
                    "\(explicitBinaryEnvironmentKey) must be an absolute path."
                )
            }
            let executable = try validateExecutable(
                URL(fileURLWithPath: explicitValue),
                label: explicitBinaryEnvironmentKey,
                forbiddenRoots: context.forbiddenRoots
            )
            resolvedExecutable = (.explicitEnvironment, executable)
        } else if let bundledServerURL = context.bundledServerURL {
            let bundleRoot = try canonicalDirectory(context.bundleRoot, label: "app bundle root")
            let executable = try validateExecutable(
                bundledServerURL,
                label: "bundled image-grid-server resource",
                requiredRoot: bundleRoot,
                forbiddenRoots: context.forbiddenRoots
            )
            resolvedExecutable = (.bundledResource, executable)
        } else {
            guard let repositoryRoot = context.repositoryRoot else {
                throw NativeRuntimeResolutionError.invalidConfiguration(
                    "No native image-grid-server is available. Set "
                        + "\(explicitBinaryEnvironmentKey) to an absolute executable, or bundle "
                        + "image-grid-server with the app."
                )
            }
            let canonicalRepositoryRoot = try validateRepositoryRoot(repositoryRoot)
            let candidates = [
                canonicalRepositoryRoot
                    .appendingPathComponent("target", isDirectory: true)
                    .appendingPathComponent("debug", isDirectory: true)
                    .appendingPathComponent(executableName, isDirectory: false),
                canonicalRepositoryRoot
                    .appendingPathComponent("target", isDirectory: true)
                    .appendingPathComponent("release", isDirectory: true)
                    .appendingPathComponent(executableName, isDirectory: false),
            ]
            guard let candidate = candidates.first(where: {
                FileManager.default.fileExists(atPath: $0.path)
            }) else {
                throw NativeRuntimeResolutionError.invalidConfiguration(
                    "No validated development image-grid-server exists under "
                        + "\(canonicalRepositoryRoot.path). Build the Rust server first."
                )
            }
            let executable = try validateExecutable(
                candidate,
                label: "development image-grid-server",
                requiredRoot: canonicalRepositoryRoot,
                forbiddenRoots: context.forbiddenRoots
            )
            resolvedExecutable = (.developmentBuild, executable)
        }

        let arguments = [
            "--bind",
            bindAddress,
            "--data-root",
            paths.dataRoot.path,
            "--server-root",
            paths.serverRoot.path,
            "--workspace-root",
            paths.workspaceRoot.path,
            "--launch-target",
            "swiftui",
        ]
        return NativeRuntimeLaunchPlan(
            source: resolvedExecutable.0,
            executableURL: resolvedExecutable.1,
            arguments: arguments,
            currentDirectoryURL: paths.workspaceRoot,
            dataRoot: paths.dataRoot,
            serverRoot: paths.serverRoot,
            workspaceRoot: paths.workspaceRoot
        )
    }

    static func expectedServerRoot(
        context: NativeRuntimeResolutionContext
    ) throws -> URL {
        try prepareRuntimePaths(context: context).serverRoot
    }

    static func developmentRepositoryRoot(sourceFilePath: String) -> URL? {
        var candidate = URL(fileURLWithPath: sourceFilePath).deletingLastPathComponent()
        for _ in 0..<8 {
            if let root = try? validateRepositoryRoot(candidate) {
                return root
            }
            let parent = candidate.deletingLastPathComponent()
            guard parent.path != candidate.path else { break }
            candidate = parent
        }
        return nil
    }

    private struct RuntimePaths {
        let dataRoot: URL
        let serverRoot: URL
        let workspaceRoot: URL
    }

    private static func prepareRuntimePaths(
        context: NativeRuntimeResolutionContext
    ) throws -> RuntimePaths {
        let dataRoot = context.applicationSupportDirectory.appendingPathComponent(
            expectedIdentity,
            isDirectory: true
        )
        do {
            try FileManager.default.createDirectory(
                at: dataRoot,
                withIntermediateDirectories: true
            )
        } catch {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "Could not create the native Application Support data root: "
                    + error.localizedDescription
            )
        }

        let canonicalDataRoot = try canonicalDirectory(dataRoot, label: "native data root")
        let serverRootCandidate = context.repositoryRoot ?? context.bundleRoot
        let canonicalServerRoot: URL
        if context.repositoryRoot != nil {
            canonicalServerRoot = try validateRepositoryRoot(serverRootCandidate)
        } else {
            canonicalServerRoot = try canonicalDirectory(
                serverRootCandidate,
                label: "native server root"
            )
        }
        return RuntimePaths(
            dataRoot: canonicalDataRoot,
            serverRoot: canonicalServerRoot,
            workspaceRoot: canonicalDataRoot
        )
    }

    private static func validateRepositoryRoot(_ url: URL) throws -> URL {
        let root = try canonicalDirectory(url, label: "native repository root")
        var cargoIsDirectory = ObjCBool(false)
        var serverCrateIsDirectory = ObjCBool(false)
        let cargoManifest = root.appendingPathComponent("Cargo.toml", isDirectory: false)
        let serverCrate = root
            .appendingPathComponent("crates", isDirectory: true)
            .appendingPathComponent("image-grid-server", isDirectory: true)
        let hasCargoManifest = FileManager.default.fileExists(
            atPath: cargoManifest.path,
            isDirectory: &cargoIsDirectory
        ) && !cargoIsDirectory.boolValue
        let hasServerCrate = FileManager.default.fileExists(
            atPath: serverCrate.path,
            isDirectory: &serverCrateIsDirectory
        ) && serverCrateIsDirectory.boolValue
        guard hasCargoManifest, hasServerCrate else {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "The development server root is not this native repository: \(root.path)"
            )
        }
        return root
    }

    private static func canonicalDirectory(_ url: URL, label: String) throws -> URL {
        guard url.isFileURL else {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "\(label) must be a local file URL."
            )
        }
        var isDirectory = ObjCBool(false)
        guard
            FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory),
            isDirectory.boolValue
        else {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "\(label) is unavailable at \(url.path)."
            )
        }
        return url.resolvingSymlinksInPath().standardizedFileURL
    }

    private static func validateExecutable(
        _ url: URL,
        label: String,
        requiredRoot: URL? = nil,
        forbiddenRoots: [URL]
    ) throws -> URL {
        guard url.isFileURL, url.pathExtension.isEmpty else {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "\(label) must identify the image-grid-server executable."
            )
        }
        var isDirectory = ObjCBool(false)
        guard
            FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory),
            !isDirectory.boolValue
        else {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "\(label) is unavailable at \(url.path)."
            )
        }
        let canonical = url.resolvingSymlinksInPath().standardizedFileURL
        guard canonical.lastPathComponent == executableName else {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "\(label) must resolve to an executable named \(executableName)."
            )
        }
        guard FileManager.default.isExecutableFile(atPath: canonical.path) else {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "\(label) is not executable at \(canonical.path)."
            )
        }
        if let requiredRoot, !contains(canonical, in: requiredRoot) {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "\(label) resolved outside the allowed native runtime root."
            )
        }
        for forbiddenRoot in forbiddenRoots {
            let canonicalForbiddenRoot = forbiddenRoot
                .resolvingSymlinksInPath()
                .standardizedFileURL
            if contains(canonical, in: canonicalForbiddenRoot) {
                throw NativeRuntimeResolutionError.invalidConfiguration(
                    "\(label) resolved inside a forbidden Electron, frozen, or plugin-cache root."
                )
            }
        }
        return canonical
    }

    private static func contains(_ child: URL, in root: URL) -> Bool {
        let childPath = child.standardizedFileURL.path
        let rootPath = root.standardizedFileURL.path
        return childPath == rootPath || childPath.hasPrefix(rootPath + "/")
    }
}

private struct NativeRuntimeHealthPayload: Decodable {
    struct Identity: Decodable {
        let app: String?
        let serverRoot: String?
        let packageName: String?
        let launchTarget: String?
    }

    let ok: Bool
    let app: String?
    let serverRoot: String?
    let packageName: String?
    let launchTarget: String?
    let identity: Identity?
}

enum NativeRuntimeHealthValidation {
    static func rejection(
        data: Data,
        expectedServerRoot: URL,
        requiredLaunchTarget: String? = nil
    ) -> String? {
        let payload: NativeRuntimeHealthPayload
        do {
            payload = try JSONDecoder().decode(NativeRuntimeHealthPayload.self, from: data)
        } catch {
            return "The native health endpoint did not return its declared JSON identity."
        }
        guard payload.ok else {
            return "The native health endpoint did not report ok=true."
        }

        let app = payload.identity?.app ?? payload.app
        let packageName = payload.identity?.packageName ?? payload.packageName
        guard
            app == NativeRuntimeResolver.expectedIdentity,
            packageName == NativeRuntimeResolver.expectedIdentity
        else {
            return "The listener on 127.0.0.1:4322 is not Codex Image Grid Native."
        }
        guard let reportedServerRoot = payload.identity?.serverRoot ?? payload.serverRoot else {
            return "The native health response did not include serverRoot."
        }
        let reportedURL = URL(fileURLWithPath: reportedServerRoot)
            .resolvingSymlinksInPath()
            .standardizedFileURL
        guard reportedURL.path == expectedServerRoot.standardizedFileURL.path else {
            return "The native health serverRoot does not match this app runtime."
        }
        if let requiredLaunchTarget {
            let launchTarget = payload.identity?.launchTarget ?? payload.launchTarget
            guard launchTarget == requiredLaunchTarget else {
                return "The launched native runtime did not report launchTarget=\(requiredLaunchTarget)."
            }
        }
        return nil
    }
}

@MainActor
final class NativeRuntimeLifecycle: ObservableObject {
    static let healthURL = URL(string: "http://127.0.0.1:4322/api/health")!
    static let healthRequestTimeout: TimeInterval = 1
    static let readinessTimeout: TimeInterval = 15

    @Published private(set) var state = NativeRuntimeLifecycleState.idle

    private var startupTask: Task<Void, Never>?
    private var ownedProcess: Process?

    func start() {
        guard startupTask == nil else { return }
        state = .checking
        startupTask = Task { [weak self] in
            guard let self else { return }
            await bootstrap()
            startupTask = nil
        }
    }

    func stop() {
        startupTask?.cancel()
        startupTask = nil
        terminateOwnedProcess()
        state = .idle
    }

    private func bootstrap() async {
        do {
            let context = try NativeRuntimeResolver.productionContext()
            let expectedServerRoot = try NativeRuntimeResolver.expectedServerRoot(context: context)
            switch await probeHealth(expectedServerRoot: expectedServerRoot) {
            case .healthy:
                state = .ready(.joined)
                return
            case let .invalid(reason):
                throw NativeRuntimeResolutionError.invalidConfiguration(reason)
            case .unavailable:
                break
            }

            let plan = try NativeRuntimeResolver.resolveLaunchPlan(context: context)
            try Task.checkCancellation()
            state = .launching
            let process = try launch(plan: plan)
            ownedProcess = process
            try await waitUntilReady(process: process, plan: plan)
            state = .ready(.launched)
        } catch is CancellationError {
            terminateOwnedProcess()
        } catch {
            terminateOwnedProcess()
            state = .failed(error.localizedDescription)
        }
    }

    private enum ProbeResult {
        case healthy
        case unavailable
        case invalid(String)
    }

    private func probeHealth(
        expectedServerRoot: URL,
        requiredLaunchTarget: String? = nil
    ) async -> ProbeResult {
        var request = URLRequest(url: Self.healthURL)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = Self.healthRequestTimeout
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse else {
                return .invalid("The native health endpoint returned an invalid response.")
            }
            guard 200..<300 ~= http.statusCode else {
                return .invalid(
                    "The native health endpoint returned HTTP \(http.statusCode)."
                )
            }
            if let rejection = NativeRuntimeHealthValidation.rejection(
                data: data,
                expectedServerRoot: expectedServerRoot,
                requiredLaunchTarget: requiredLaunchTarget
            ) {
                return .invalid(rejection)
            }
            return .healthy
        } catch let error as URLError {
            switch error.code {
            case .cannotConnectToHost, .cannotFindHost, .networkConnectionLost:
                return .unavailable
            default:
                return .invalid(
                    "The native health request failed: \(error.localizedDescription)"
                )
            }
        } catch {
            return .invalid("The native health request failed: \(error.localizedDescription)")
        }
    }

    private func launch(plan: NativeRuntimeLaunchPlan) throws -> Process {
        let process = Process()
        process.executableURL = plan.executableURL
        process.arguments = plan.arguments
        process.currentDirectoryURL = plan.currentDirectoryURL
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            return process
        } catch {
            throw NativeRuntimeResolutionError.invalidConfiguration(
                "Could not launch Image Grid Native with \(plan.source.rawValue): "
                    + error.localizedDescription
            )
        }
    }

    private func waitUntilReady(
        process: Process,
        plan: NativeRuntimeLaunchPlan
    ) async throws {
        let deadline = Date().addingTimeInterval(Self.readinessTimeout)
        while Date() < deadline {
            try Task.checkCancellation()
            if !process.isRunning {
                throw NativeRuntimeResolutionError.invalidConfiguration(
                    "The native runtime exited before health became ready "
                        + "(status \(process.terminationStatus))."
                )
            }
            switch await probeHealth(
                expectedServerRoot: plan.serverRoot,
                requiredLaunchTarget: "swiftui"
            ) {
            case .healthy:
                return
            case .unavailable:
                try await Task.sleep(for: .milliseconds(100))
            case let .invalid(reason):
                throw NativeRuntimeResolutionError.invalidConfiguration(reason)
            }
        }
        throw NativeRuntimeResolutionError.invalidConfiguration(
            "Image Grid Native did not become healthy on 127.0.0.1:4322 within "
                + "\(Int(Self.readinessTimeout)) seconds."
        )
    }

    private func terminateOwnedProcess() {
        guard let process = ownedProcess else { return }
        ownedProcess = nil
        if process.isRunning {
            process.terminate()
        }
    }
}
