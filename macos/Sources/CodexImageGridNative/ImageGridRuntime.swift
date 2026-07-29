import AppKit
import Combine
import Foundation
import ImageIO
import UniformTypeIdentifiers

enum RuntimeConnectionState: Sendable {
    case disconnected
    case idle
    case starting
    case ready
    case error
}

struct ImageGridTimestamp: Codable, Hashable, Sendable {
    let milliseconds: Int64

    init(milliseconds: Int64) {
        self.milliseconds = milliseconds
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let value = try? container.decode(Int64.self) {
            milliseconds = value
            return
        }
        if let value = try? container.decode(Double.self) {
            milliseconds = Int64(value)
            return
        }
        let value = try container.decode(String.self)
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let basic = ISO8601DateFormatter()
        basic.formatOptions = [.withInternetDateTime]
        let date = fractional.date(from: value) ?? basic.date(from: value)
        milliseconds = date.map { Int64($0.timeIntervalSince1970 * 1_000) } ?? 0
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(milliseconds)
    }
}

struct ImageGridJob: Codable, Hashable, Identifiable, Sendable {
    let id: String
    var runId: String?
    var engine: String?
    var model: String?
    var prompt: String?
    var referencePremise: String?
    var mood: String?
    var promptIndex: Int?
    var promptTotal: Int?
    var variant: Int?
    var total: Int?
    var filename: String?
    var outputPath: String?
    var aspectRatio: String?
    var referenceImagePath: String?
    var referenceImageUrl: String?
    var manifestPath: String?
    var manifestUrl: String?
    var manifestViewUrl: String?
    var handoffPath: String?
    var handoffUrl: String?
    var handoffViewUrl: String?
    var outputFormat: String?
    var status: String
    var statusText: String?
    var imageUrl: String?
    var log: String?
    var errorCode: String?
    var errorMessage: String?
    var diagnosticLog: String?
    var createdAt: ImageGridTimestamp?
    var updatedAt: ImageGridTimestamp?

    var isActive: Bool {
        ["queued", "starting", "running"].contains(status)
    }

    var eventTime: Int64 {
        max(updatedAt?.milliseconds ?? 0, createdAt?.milliseconds ?? 0)
    }

    init(
        id: String,
        runId: String? = nil,
        status: String,
        statusText: String? = nil,
        prompt: String? = nil,
        promptIndex: Int? = nil,
        promptTotal: Int? = nil,
        variant: Int? = nil,
        total: Int? = nil,
        outputPath: String? = nil,
        imageUrl: String? = nil,
        updatedAt: ImageGridTimestamp? = nil
    ) {
        self.id = id
        self.runId = runId
        engine = nil
        model = nil
        self.prompt = prompt
        referencePremise = nil
        mood = nil
        self.promptIndex = promptIndex
        self.promptTotal = promptTotal
        self.variant = variant
        self.total = total
        filename = nil
        self.outputPath = outputPath
        aspectRatio = nil
        referenceImagePath = nil
        referenceImageUrl = nil
        manifestPath = nil
        manifestUrl = nil
        manifestViewUrl = nil
        handoffPath = nil
        handoffUrl = nil
        handoffViewUrl = nil
        outputFormat = nil
        self.status = status
        self.statusText = statusText
        self.imageUrl = imageUrl
        log = nil
        errorCode = nil
        errorMessage = nil
        diagnosticLog = nil
        createdAt = nil
        self.updatedAt = updatedAt
    }
}

struct ImageGridGenerationRequest: Encodable, Equatable, Sendable {
    let prompt: String
    let prompts: [String]?
    let referencePremise: String
    let mood: String
    let engine: String
    let count: Int
    let aspectRatio: String
    let referenceImagePath: String?
}

struct ImageGridRunEnvelope: Decodable, Sendable {
    let runId: String?
    let jobs: [ImageGridJob]?
    let outputs: [ImageGridJob]?
    let manifestViewUrl: String?
    let handoffViewUrl: String?
    let server: ImageGridRuntimeIdentity?

    var hydratedJobs: [ImageGridJob] {
        let candidates = (jobs ?? []) + (outputs ?? [])
        return candidates.map { candidate in
            var job = candidate
            if job.runId == nil {
                job.runId = runId
            }
            if job.manifestViewUrl == nil {
                job.manifestViewUrl = manifestViewUrl
            }
            if job.handoffViewUrl == nil {
                job.handoffViewUrl = handoffViewUrl
            }
            return job
        }
    }
}

private struct ImageGridRunListResponse: Decodable {
    let data: [ImageGridRunEnvelope]
}

private struct ImageGridRunEvent: Decodable {
    let jobs: [ImageGridJob]
}

private struct ImageGridDeleteRunsRequest: Encodable {
    let runIds: [String]
}

struct ImageGridDeleteRunFailure: Decodable, Equatable, Sendable {
    let runId: String
    let error: String
}

struct ImageGridDeleteRunsResponse: Decodable, Equatable, Sendable {
    let ok: Bool
    let deletedRunIds: [String]
    let deletedJobCount: Int
    let failures: [ImageGridDeleteRunFailure]
}

private struct ImageGridRunsDeletedEvent: Decodable {
    let runIds: [String]
}

struct ImageGridRuntimeIdentity: Decodable, Equatable, Sendable {
    let app: String?
    let serverRoot: String?
    let packageName: String?
    let packageVersion: String?
    let packageRootKind: String?
    let launchTarget: String?
}

struct ImageGridHealthResponse: Decodable {
    let ok: Bool
    let app: String?
    let serverRoot: String?
    let packageName: String?
    let packageVersion: String?
    let packageRootKind: String?
    let launchTarget: String?
    let generatedDir: String?
    let appServerImageReady: Bool?
    let appServerImageDiagnostics: ImageGridDiagnostics?
    let identity: ImageGridRuntimeIdentity?

    var topLevelIdentity: ImageGridRuntimeIdentity {
        ImageGridRuntimeIdentity(
            app: app,
            serverRoot: serverRoot,
            packageName: packageName,
            packageVersion: packageVersion,
            packageRootKind: packageRootKind,
            launchTarget: launchTarget
        )
    }
}

struct ImageGridDiagnostics: Decodable {
    let ready: Bool?
    let status: String?
}

private struct ImageGridPreflightResponse: Decodable {
    let ok: Bool?
    let appServerImageReady: Bool?
    let diagnostics: ImageGridDiagnostics?
}

private struct ImageGridAnalysisRequest: Encodable {
    let referenceImagePath: String
}

private struct ImageGridAnalysisResponse: Decodable {
    let premise: String?
}

struct ImageGridSSEEvent: Equatable, Sendable {
    let name: String
    let data: Data
}

struct ImageGridSSEParser {
    private var eventName = "message"
    private var dataLines: [String] = []

    mutating func consume(line: String) -> ImageGridSSEEvent? {
        if line.isEmpty {
            guard !dataLines.isEmpty else {
                eventName = "message"
                return nil
            }
            defer {
                eventName = "message"
                dataLines.removeAll(keepingCapacity: true)
            }
            return ImageGridSSEEvent(
                name: eventName,
                data: Data(dataLines.joined(separator: "\n").utf8)
            )
        }
        if line.hasPrefix(":") {
            return nil
        }
        if line.hasPrefix("event:") {
            eventName = String(line.dropFirst(6)).trimmingCharacters(in: .whitespaces)
        } else if line.hasPrefix("data:") {
            var value = String(line.dropFirst(5))
            if value.hasPrefix(" ") {
                value.removeFirst()
            }
            dataLines.append(value)
        }
        return nil
    }
}

struct ImageGridAPIError: LocalizedError, Equatable, Sendable {
    let message: String

    var errorDescription: String? { message }
}

enum ImageGridRuntimeIdentityValidation {
    static let expectedIdentity = "codex-image-grid"
    static let expectedPackageVersion = "0.2.4"

    static func validate(_ response: ImageGridHealthResponse) throws -> ImageGridRuntimeIdentity {
        guard response.ok else {
            throw ImageGridAPIError(
                message: "Disconnected: the local Image Grid health endpoint did not report ok=true."
            )
        }
        return try validate(
            topLevel: response.topLevelIdentity,
            nested: response.identity
        )
    }

    static func validate(_ identity: ImageGridRuntimeIdentity?) throws
        -> ImageGridRuntimeIdentity
    {
        try validate(
            topLevel: identity ?? ImageGridRuntimeIdentity(
                app: nil,
                serverRoot: nil,
                packageName: nil,
                packageVersion: nil,
                packageRootKind: nil,
                launchTarget: nil
            ),
            nested: nil
        )
    }

    private static func validate(
        topLevel: ImageGridRuntimeIdentity,
        nested: ImageGridRuntimeIdentity?
    ) throws -> ImageGridRuntimeIdentity {
        let app = nested?.app ?? topLevel.app
        let packageName = nested?.packageName ?? topLevel.packageName
        guard app == expectedIdentity, packageName == expectedIdentity else {
            throw ImageGridAPIError(
                message: "Disconnected: the listener is not Codex Image Grid Native "
                    + "(expected app and packageName \(expectedIdentity))."
            )
        }
        let packageVersion = nested?.packageVersion ?? topLevel.packageVersion
        guard packageVersion == expectedPackageVersion else {
            throw ImageGridAPIError(
                message: "Disconnected: Codex Image Grid Native packageVersion does not match "
                    + "\(expectedPackageVersion)."
            )
        }
        let packageRootKind = nested?.packageRootKind ?? topLevel.packageRootKind
        guard packageRootKind == "source" || packageRootKind == "packaged" else {
            throw ImageGridAPIError(
                message: "Disconnected: Codex Image Grid Native did not report a supported "
                    + "source or packaged runtime root."
            )
        }
        let launchTarget = nested?.launchTarget ?? topLevel.launchTarget
        guard launchTarget == "swiftui" else {
            throw ImageGridAPIError(
                message: "Disconnected: Codex Image Grid Native did not report "
                    + "launchTarget=swiftui."
            )
        }

        let serverRoot = (nested?.serverRoot ?? topLevel.serverRoot)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard let serverRoot,
              !serverRoot.isEmpty,
              (serverRoot as NSString).isAbsolutePath
        else {
            throw ImageGridAPIError(
                message: "Disconnected: Codex Image Grid Native did not report "
                    + "a nonempty absolute serverRoot."
            )
        }
        return ImageGridRuntimeIdentity(
            app: app,
            serverRoot: serverRoot,
            packageName: packageName,
            packageVersion: packageVersion,
            packageRootKind: packageRootKind,
            launchTarget: launchTarget
        )
    }
}

struct ImageGridAPIClient: Sendable {
    static let defaultBaseURL = URL(string: "http://127.0.0.1:4322")!
    static let analysisTimeout: TimeInterval = 185

    let baseURL: URL
    private let session: URLSession
    private let decoder = JSONDecoder()
    private let encoder = JSONEncoder()

    init(baseURL: URL = Self.defaultBaseURL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    func health() async throws -> (
        state: RuntimeConnectionState,
        generatedDirectory: URL?
    ) {
        let response: ImageGridHealthResponse = try await send(path: "/api/health")
        _ = try ImageGridRuntimeIdentityValidation.validate(response)
        let diagnostics = response.appServerImageDiagnostics
        let ready = response.appServerImageReady == true || diagnostics?.ready == true
            || diagnostics?.status == "ready"
        let failed = ["error", "failed", "stopped", "exited"].contains(diagnostics?.status ?? "")
        let directory = response.generatedDir.map { URL(fileURLWithPath: $0) }
        return (failed ? .error : (ready ? .ready : .idle), directory)
    }

    func preflight() async throws {
        _ = try await health()
        let response: ImageGridPreflightResponse = try await send(
            path: "/api/preflight/app-server-image",
            method: "POST"
        )
        let ready = response.appServerImageReady == true || response.diagnostics?.ready == true
            || response.diagnostics?.status == "ready"
        guard response.ok != false, ready else {
            throw ImageGridAPIError(message: "Codex App Server preflight did not become ready.")
        }
    }

    func generate(
        request: ImageGridGenerationRequest,
        batch: Bool
    ) async throws -> ImageGridRunEnvelope {
        _ = try await health()
        let response: ImageGridRunEnvelope = try await send(
            path: batch ? "/api/run-batch" : "/api/run",
            method: "POST",
            body: request
        )
        _ = try ImageGridRuntimeIdentityValidation.validate(response.server)
        return response
    }

    func runs() async throws -> [ImageGridRunEnvelope] {
        _ = try await health()
        let response: ImageGridRunListResponse = try await send(path: "/api/runs")
        for run in response.data {
            _ = try ImageGridRuntimeIdentityValidation.validate(run.server)
        }
        return response.data
    }

    func run(id: String) async throws -> ImageGridRunEnvelope {
        let response: ImageGridRunEnvelope = try await send(path: "/api/runs/\(id)")
        _ = try ImageGridRuntimeIdentityValidation.validate(response.server)
        return response
    }

    func deleteRuns(_ runIDs: [String]) async throws -> ImageGridDeleteRunsResponse {
        _ = try await health()
        return try await send(
            path: "/api/delete-runs",
            method: "POST",
            body: ImageGridDeleteRunsRequest(runIds: runIDs)
        )
    }

    func analyze(referenceImagePath: String) async throws -> String {
        _ = try await health()
        let request = try analysisRequest(referenceImagePath: referenceImagePath)
        let response: ImageGridAnalysisResponse = try await send(request: request)
        guard let premise = response.premise?.trimmingCharacters(in: .whitespacesAndNewlines),
              !premise.isEmpty
        else {
            throw ImageGridAPIError(message: "Reference analysis returned no premise.")
        }
        return premise
    }

    func analysisRequest(referenceImagePath: String) throws -> URLRequest {
        try makeRequest(
            path: "/api/analyze-reference",
            method: "POST",
            bodyData: encoder.encode(
                ImageGridAnalysisRequest(referenceImagePath: referenceImagePath)
            ),
            timeout: Self.analysisTimeout
        )
    }

    func consumeEvents(
        onEvent: @escaping @Sendable (ImageGridSSEEvent) async -> Void
    ) async throws {
        _ = try await health()
        var request = URLRequest(url: endpoint("/events"))
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 60 * 60
        let (bytes, response) = try await session.bytes(for: request)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw ImageGridAPIError(message: "The Image Grid event stream is unavailable.")
        }
        var parser = ImageGridSSEParser()
        for try await line in bytes.lines {
            try Task.checkCancellation()
            if let event = parser.consume(line: line) {
                await onEvent(event)
            }
        }
        throw ImageGridAPIError(message: "The Image Grid event stream closed.")
    }

    func resolvedURL(_ value: String?) -> URL? {
        guard let value, !value.isEmpty else { return nil }
        return URL(string: value, relativeTo: baseURL)?.absoluteURL
    }

    private func endpoint(_ path: String) -> URL {
        URL(string: path, relativeTo: baseURL)!.absoluteURL
    }

    private func send<Response: Decodable>(
        path: String,
        method: String = "GET"
    ) async throws -> Response {
        try await send(path: path, method: method, bodyData: nil)
    }

    private func send<Response: Decodable, Body: Encodable>(
        path: String,
        method: String,
        body: Body
    ) async throws -> Response {
        try await send(path: path, method: method, bodyData: encoder.encode(body))
    }

    private func send<Response: Decodable>(
        path: String,
        method: String,
        bodyData: Data?
    ) async throws -> Response {
        let request = try makeRequest(path: path, method: method, bodyData: bodyData)
        return try await send(request: request)
    }

    private func makeRequest(
        path: String,
        method: String,
        bodyData: Data?,
        timeout: TimeInterval = 20
    ) throws -> URLRequest {
        var request = URLRequest(url: endpoint(path))
        request.httpMethod = method
        request.timeoutInterval = timeout
        request.cachePolicy = .reloadIgnoringLocalCacheData
        if let bodyData {
            request.httpBody = bodyData
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        return request
    }

    private func send<Response: Decodable>(request: URLRequest) async throws -> Response {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw ImageGridAPIError(message: "The Image Grid server returned an invalid response.")
        }
        guard 200..<300 ~= http.statusCode else {
            throw ImageGridAPIError(message: Self.errorMessage(from: data, status: http.statusCode))
        }
        do {
            return try decoder.decode(Response.self, from: data)
        } catch {
            throw ImageGridAPIError(message: "The Image Grid response could not be decoded.")
        }
    }

    private static func errorMessage(from data: Data, status: Int) -> String {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return "Image Grid request failed (\(status))."
        }
        if let error = object["error"] as? String, !error.isEmpty {
            return error
        }
        if let diagnostics = object["diagnostics"] as? [String: Any],
           let error = diagnostics["error"] as? [String: Any],
           let message = error["message"] as? String,
           !message.isEmpty
        {
            return message
        }
        return "Image Grid request failed (\(status))."
    }
}

enum ImageGridJobSelection {
    static let maximumCompletedJobs = 96

    static func visible(
        jobs: some Sequence<ImageGridJob>,
        completedLimit: Int?,
        showFailed: Bool
    ) -> [ImageGridJob] {
        let all = Array(jobs)
        let active = all.filter(\.isActive).sorted(by: orderedBefore)
        var terminal = all.filter { !$0.isActive && (showFailed || $0.status != "error") }
            .sorted(by: orderedBefore)
        if let completedLimit {
            terminal = Array(terminal.prefix(max(0, min(maximumCompletedJobs, completedLimit))))
        }
        return active + terminal
    }

    static func retained(
        jobs: some Sequence<ImageGridJob>,
        completedLimit: Int?
    ) -> [ImageGridJob] {
        let all = Array(jobs)
        guard let completedLimit else {
            return all
        }

        let boundedLimit = max(0, min(maximumCompletedJobs, completedLimit))
        let active = all.filter(\.isActive)
        let visibleWithFailures = visible(
            jobs: all,
            completedLimit: boundedLimit,
            showFailed: true
        )
        let visibleWithoutFailures = visible(
            jobs: all,
            completedLimit: boundedLimit,
            showFailed: false
        )

        var retainedIDs: Set<String> = []
        var retained: [ImageGridJob] = []
        for job in active + visibleWithFailures + visibleWithoutFailures {
            if retainedIDs.insert(job.id).inserted {
                retained.append(job)
            }
        }
        return retained
    }

    static func orderedBefore(_ lhs: ImageGridJob, _ rhs: ImageGridJob) -> Bool {
        if lhs.isActive != rhs.isActive {
            return lhs.isActive
        }
        if (lhs.status == "error") != (rhs.status == "error") {
            return lhs.status == "error"
        }
        if lhs.eventTime != rhs.eventTime {
            return lhs.eventTime > rhs.eventTime
        }
        if (lhs.promptIndex ?? 1) != (rhs.promptIndex ?? 1) {
            return (lhs.promptIndex ?? 1) < (rhs.promptIndex ?? 1)
        }
        if (lhs.variant ?? 1) != (rhs.variant ?? 1) {
            return (lhs.variant ?? 1) < (rhs.variant ?? 1)
        }
        return lhs.id < rhs.id
    }
}

enum ImageGridRunSelection {
    static func selectableRunIDs(jobs: some Sequence<ImageGridJob>) -> Set<String> {
        grouped(jobs: jobs).reduce(into: Set<String>()) { selected, entry in
            if !entry.value.contains(where: \.isActive) {
                selected.insert(entry.key)
            }
        }
    }

    static func failedRunIDs(jobs: some Sequence<ImageGridJob>) -> Set<String> {
        grouped(jobs: jobs).reduce(into: Set<String>()) { selected, entry in
            if !entry.value.contains(where: \.isActive),
               entry.value.contains(where: { $0.status == "error" })
            {
                selected.insert(entry.key)
            }
        }
    }

    static func affectedJobCount(
        runIDs: Set<String>,
        jobs: some Sequence<ImageGridJob>
    ) -> Int {
        jobs.filter { job in
            job.runId.map(runIDs.contains) == true
        }.count
    }

    static func isSelectable(_ job: ImageGridJob, jobs: some Sequence<ImageGridJob>) -> Bool {
        guard let runID = job.runId else { return false }
        return selectableRunIDs(jobs: jobs).contains(runID)
    }

    private static func grouped(
        jobs: some Sequence<ImageGridJob>
    ) -> [String: [ImageGridJob]] {
        Dictionary(grouping: jobs.compactMap { job in
            job.runId.map { ($0, job) }
        }, by: { $0.0 })
            .mapValues { entries in entries.map { $0.1 } }
    }
}

struct ImageGridFailureNoticeTracker {
    private var observedActiveJobIDs: Set<String> = []
    private var unacknowledgedFailureIDs: Set<String> = []

    var unacknowledgedCount: Int {
        unacknowledgedFailureIDs.count
    }

    mutating func observe(previous: ImageGridJob?, incoming: ImageGridJob) {
        if incoming.isActive {
            observedActiveJobIDs.insert(incoming.id)
            return
        }

        if incoming.status == "error",
           previous?.isActive == true || observedActiveJobIDs.contains(incoming.id)
        {
            unacknowledgedFailureIDs.insert(incoming.id)
        }
        observedActiveJobIDs.remove(incoming.id)
    }

    mutating func acknowledgeFailures() {
        unacknowledgedFailureIDs.removeAll()
    }

    mutating func removeJobs(withIDs jobIDs: Set<String>) {
        observedActiveJobIDs.subtract(jobIDs)
        unacknowledgedFailureIDs.subtract(jobIDs)
    }

    mutating func retainJobs(withIDs jobIDs: Set<String>) {
        observedActiveJobIDs.formIntersection(jobIDs)
        unacknowledgedFailureIDs.formIntersection(jobIDs)
    }
}

@MainActor
final class ImageGridStore: ObservableObject {
    @Published private(set) var jobs: [String: ImageGridJob] = [:]
    @Published private(set) var deletionJobs: [String: ImageGridJob] = [:]
    @Published private(set) var runtimeState = RuntimeConnectionState.disconnected
    @Published private(set) var generatedDirectory: URL?
    @Published private(set) var isSubmitting = false
    @Published private(set) var isAnalyzing = false
    @Published private(set) var isDeletingRuns = false
    @Published private(set) var unacknowledgedFailureCount = 0
    @Published var generationMessage: String?
    @Published var referenceAnalysisMessage: String?
    @Published var deletionMessage: String?

    let client: ImageGridAPIClient
    private var lifecycleTask: Task<Void, Never>?
    private let activeRunReconciliationInterval: Duration
    private var activeRunReconciliationTask: Task<Void, Never>?
    private var activeRunReconciliationToken: UUID?
    private var clearedBefore: Int64 = 0
    private var clearedJobIDs: Set<String> = []
    private var retainedCompletedLimit: Int? = ImageGridJobSelection.maximumCompletedJobs
    private var deletionWorkspaceActive = false
    private var connectivityMessage: String?
    private var failureNoticeTracker = ImageGridFailureNoticeTracker()

    init(
        client: ImageGridAPIClient = ImageGridAPIClient(),
        activeRunReconciliationInterval: Duration = .milliseconds(500)
    ) {
        self.client = client
        self.activeRunReconciliationInterval = activeRunReconciliationInterval
    }

    func synchronize(with nativeState: NativeRuntimeLifecycleState) {
        switch nativeState {
        case .idle:
            stop()
            runtimeState = .disconnected
        case .checking, .launching:
            runtimeState = .starting
        case .ready:
            start()
        case let .failed(message):
            stop()
            runtimeState = .disconnected
            recordConnectivityFailure(message)
        }
    }

    func start() {
        if lifecycleTask == nil {
            lifecycleTask = Task { [weak self] in
                guard let self else { return }
                await consumeEventsWithRecovery()
                lifecycleTask = nil
            }
        }
        updateActiveRunReconciliation()
    }

    func stop() {
        lifecycleTask?.cancel()
        lifecycleTask = nil
        activeRunReconciliationToken = nil
        activeRunReconciliationTask?.cancel()
        activeRunReconciliationTask = nil
    }

    func refreshHealth() async {
        runtimeState = .starting
        do {
            let health = try await client.health()
            runtimeState = health.state
            generatedDirectory = health.generatedDirectory
            clearConnectivityFailure()
        } catch {
            runtimeState = .disconnected
            recordConnectivityFailure(error.localizedDescription)
        }
    }

    func hydrateRuns() async {
        do {
            for run in try await client.runs() {
                merge(run.hydratedJobs)
            }
        } catch {
            if jobs.isEmpty {
                generationMessage = error.localizedDescription
            }
        }
    }

    func generate(request: ImageGridGenerationRequest, batch: Bool) async -> Bool {
        guard !isSubmitting else { return false }
        isSubmitting = true
        generationMessage = nil
        defer { isSubmitting = false }

        do {
            if request.engine == "app-server-image" {
                runtimeState = .starting
                try await client.preflight()
                runtimeState = .ready
            }
            let run = try await client.generate(request: request, batch: batch)
            merge(run.hydratedJobs)
            await refreshHealth()
            if lifecycleTask == nil {
                start()
            }
            return true
        } catch {
            runtimeState = request.engine == "app-server-image" ? .error : runtimeState
            generationMessage = error.localizedDescription
            return false
        }
    }

    func analyze(reference: ImageGridReference) async -> String? {
        guard !isAnalyzing else { return nil }
        isAnalyzing = true
        referenceAnalysisMessage = nil
        defer { isAnalyzing = false }
        do {
            return try await client.analyze(referenceImagePath: reference.url.path)
        } catch {
            referenceAnalysisMessage = error.localizedDescription
            return nil
        }
    }

    func deleteRuns(_ runIDs: Set<String>) async -> ImageGridDeleteRunsResponse? {
        guard !isDeletingRuns else { return nil }
        let selectionJobs = deletionWorkspaceActive ? deletionJobs.values : jobs.values
        let selectableRunIDs = ImageGridRunSelection.selectableRunIDs(jobs: selectionJobs)
        let requestedRunIDs = runIDs.intersection(selectableRunIDs).sorted()
        guard !requestedRunIDs.isEmpty else { return nil }

        isDeletingRuns = true
        deletionMessage = nil
        defer { isDeletingRuns = false }
        do {
            let response = try await client.deleteRuns(requestedRunIDs)
            removeRuns(withIDs: Set(response.deletedRunIds))
            if !response.failures.isEmpty {
                deletionMessage = response.failures
                    .map { "\($0.runId): \($0.error)" }
                    .joined(separator: "\n")
            }
            return response
        } catch {
            deletionMessage = error.localizedDescription
            return nil
        }
    }

    func beginDeletionWorkspace() {
        deletionWorkspaceActive = true
        deletionJobs = jobs
        deletionMessage = nil
    }

    func hydrateDeletionWorkspace() async {
        guard deletionWorkspaceActive else { return }
        do {
            let hydrated = try await client.runs().flatMap(\.hydratedJobs)
            guard deletionWorkspaceActive else { return }
            mergeDeletionJobs(hydrated)
        } catch {
            guard deletionWorkspaceActive else { return }
            deletionMessage = error.localizedDescription
        }
    }

    func endDeletionWorkspace() {
        deletionWorkspaceActive = false
        deletionJobs.removeAll()
        deletionMessage = nil
    }

    func visibleJobs(resultLimit: ResultLimit, showFailed: Bool) -> [ImageGridJob] {
        ImageGridJobSelection.visible(
            jobs: jobs.values,
            completedLimit: resultLimit.completedLimit,
            showFailed: showFailed
        )
    }

    func applyRetentionMode(resultLimit: ResultLimit) {
        retainedCompletedLimit = resultLimit == .all
            ? nil
            : ImageGridJobSelection.maximumCompletedJobs
        pruneRetainedJobs()
    }

    var counts: (done: Int, running: Int, failed: Int) {
        (
            jobs.values.filter { $0.status == "done" }.count,
            jobs.values.filter(\.isActive).count,
            jobs.values.filter { $0.status == "error" }.count
        )
    }

    var hasTerminalJobs: Bool {
        jobs.values.contains { !$0.isActive }
    }

    func clearTerminalJobs() {
        clearedBefore = Int64(Date().timeIntervalSince1970 * 1_000)
        let terminal = jobs.values.filter { !$0.isActive }
        let terminalIDs = Set(terminal.map(\.id))
        for job in terminal {
            clearedJobIDs.insert(job.id)
            jobs.removeValue(forKey: job.id)
        }
        failureNoticeTracker.removeJobs(withIDs: terminalIDs)
        synchronizeFailureNoticeCount()
        if clearedJobIDs.count > 512 {
            clearedJobIDs = Set(clearedJobIDs.suffix(512))
        }
    }

    func acknowledgeFailureNotice() {
        failureNoticeTracker.acknowledgeFailures()
        synchronizeFailureNoticeCount()
    }

    func openGeneratedDirectory() {
        guard let generatedDirectory else {
            generationMessage = "The generated image directory is unavailable."
            return
        }
        NSWorkspace.shared.open(generatedDirectory)
    }

    func reveal(_ job: ImageGridJob) {
        guard let path = job.outputPath, !path.isEmpty else {
            generationMessage = "This result does not expose a generated file path."
            return
        }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    func openArtifact(_ value: String?) {
        guard let url = client.resolvedURL(value) else {
            generationMessage = "This artifact is not available."
            return
        }
        NSWorkspace.shared.open(url)
    }

    func copyPrompt(_ prompt: String?) {
        guard let prompt, !prompt.isEmpty else { return }
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(prompt, forType: .string)
    }

    private func consumeEventsWithRecovery() async {
        var failures = 0
        while !Task.isCancelled {
            do {
                try await connectAndHydrate()
                failures = 0
                try await client.consumeEvents { [weak self] event in
                    await self?.receive(event)
                }
            } catch is CancellationError {
                return
            } catch {
                failures = min(5, failures + 1)
                runtimeState = .disconnected
                recordConnectivityFailure(error.localizedDescription)
                let delay = Double(min(16, 1 << (failures - 1)))
                do {
                    try await Task.sleep(for: .seconds(delay))
                } catch {
                    return
                }
            }
        }
    }

    private func connectAndHydrate() async throws {
        let health = try await client.health()
        let runs = try await client.runs()
        runtimeState = health.state
        generatedDirectory = health.generatedDirectory
        for run in runs {
            merge(run.hydratedJobs)
        }
        clearConnectivityFailure()
    }

    private func receive(_ event: ImageGridSSEEvent) {
        if case .ready = runtimeState {
            // Preserve App Server readiness while the live stream is healthy.
        } else {
            runtimeState = .idle
        }
        clearConnectivityFailure()
        let decoder = JSONDecoder()
        switch event.name {
        case "snapshot":
            if let snapshot = try? decoder.decode([ImageGridJob].self, from: event.data) {
                merge(snapshot)
            }
        case "run":
            if let run = try? decoder.decode(ImageGridRunEvent.self, from: event.data) {
                merge(run.jobs)
            }
        case "job":
            if let job = try? decoder.decode(ImageGridJob.self, from: event.data) {
                merge([job])
            }
        case "runs-deleted":
            if let deletion = try? decoder.decode(
                ImageGridRunsDeletedEvent.self,
                from: event.data
            ) {
                removeRuns(withIDs: Set(deletion.runIds))
            }
        default:
            break
        }
    }

    private func recordConnectivityFailure(_ message: String) {
        connectivityMessage = message
        generationMessage = message
    }

    private func clearConnectivityFailure() {
        if generationMessage == connectivityMessage {
            generationMessage = nil
        }
        connectivityMessage = nil
    }

    private func merge(_ incoming: [ImageGridJob]) {
        mergeDeletionJobs(incoming)
        var mergedJobs = jobs
        for job in incoming {
            let existing = mergedJobs[job.id]
            if !job.isActive, existing?.isActive != true {
                if clearedJobIDs.contains(job.id) {
                    continue
                }
                if clearedBefore > 0, job.eventTime > 0, job.eventTime <= clearedBefore {
                    continue
                }
            }
            if let existing, existing.eventTime > job.eventTime {
                continue
            }
            if existing == job {
                continue
            }
            failureNoticeTracker.observe(previous: existing, incoming: job)
            mergedJobs[job.id] = job
        }
        let retained = ImageGridJobSelection.retained(
            jobs: mergedJobs.values,
            completedLimit: retainedCompletedLimit
        )
        let retainedIDs = Set(retained.map(\.id))
        mergedJobs = mergedJobs.filter { retainedIDs.contains($0.key) }
        failureNoticeTracker.retainJobs(withIDs: retainedIDs)
        synchronizeFailureNoticeCount()
        guard mergedJobs != jobs else { return }

        jobs = mergedJobs
        updateActiveRunReconciliation()
    }

    private func mergeDeletionJobs(_ incoming: [ImageGridJob]) {
        guard deletionWorkspaceActive else { return }
        var mergedJobs = deletionJobs
        for job in incoming {
            if let existing = mergedJobs[job.id], existing.eventTime > job.eventTime {
                continue
            }
            mergedJobs[job.id] = job
        }
        if mergedJobs != deletionJobs {
            deletionJobs = mergedJobs
        }
    }

    private func removeRuns(withIDs runIDs: Set<String>) {
        guard !runIDs.isEmpty else { return }
        let removedJobIDs = Set(
            (Array(jobs.values) + Array(deletionJobs.values))
                .filter { job in
                    job.runId.map(runIDs.contains) == true
                }
                .map(\.id)
        )
        clearedJobIDs.formUnion(removedJobIDs)
        jobs = jobs.filter { _, job in
            job.runId.map { !runIDs.contains($0) } ?? true
        }
        deletionJobs = deletionJobs.filter { _, job in
            job.runId.map { !runIDs.contains($0) } ?? true
        }
        failureNoticeTracker.removeJobs(withIDs: removedJobIDs)
        synchronizeFailureNoticeCount()
        if clearedJobIDs.count > 512 {
            clearedJobIDs = Set(clearedJobIDs.suffix(512))
        }
        updateActiveRunReconciliation()
    }

    private func pruneRetainedJobs() {
        let retained = ImageGridJobSelection.retained(
            jobs: jobs.values,
            completedLimit: retainedCompletedLimit
        )
        let retainedIDs = Set(retained.map(\.id))
        let prunedJobs = jobs.filter { retainedIDs.contains($0.key) }
        failureNoticeTracker.retainJobs(withIDs: retainedIDs)
        synchronizeFailureNoticeCount()
        if prunedJobs != jobs {
            jobs = prunedJobs
        }
    }

    private func synchronizeFailureNoticeCount() {
        let count = failureNoticeTracker.unacknowledgedCount
        if unacknowledgedFailureCount != count {
            unacknowledgedFailureCount = count
        }
    }

    private func updateActiveRunReconciliation() {
        guard jobs.values.contains(where: \.isActive) else {
            activeRunReconciliationToken = nil
            activeRunReconciliationTask?.cancel()
            activeRunReconciliationTask = nil
            return
        }
        guard activeRunReconciliationTask == nil else { return }

        let token = UUID()
        activeRunReconciliationToken = token
        activeRunReconciliationTask = Task { [weak self] in
            guard let self else { return }
            await reconcileActiveRuns(token: token)
        }
    }

    private func reconcileActiveRuns(token: UUID) async {
        defer {
            if activeRunReconciliationToken == token {
                activeRunReconciliationToken = nil
                activeRunReconciliationTask = nil
            }
        }

        while !Task.isCancelled, activeRunReconciliationToken == token {
            do {
                try await Task.sleep(for: activeRunReconciliationInterval)
            } catch {
                return
            }
            guard !Task.isCancelled, activeRunReconciliationToken == token else {
                return
            }

            let runIDs = Set(
                jobs.values
                    .filter(\.isActive)
                    .compactMap(\.runId)
            ).sorted()
            guard !runIDs.isEmpty else { return }

            for runID in runIDs {
                guard !Task.isCancelled, activeRunReconciliationToken == token else {
                    return
                }
                do {
                    let run = try await client.run(id: runID)
                    guard !Task.isCancelled, activeRunReconciliationToken == token else {
                        return
                    }
                    merge(run.hydratedJobs)
                } catch is CancellationError {
                    return
                } catch {
                    continue
                }
            }
        }
    }
}

struct ImageGridReferenceDimensions: Equatable, Sendable {
    let width: Int
    let height: Int
}

enum ImageGridReferencePreparationError: Error, Equatable, Sendable {
    case unsupportedType
    case tooLarge
    case unsafeDimensions
    case decodeFailed
    case preparationFailed
}

enum ImageGridReferencePolicy {
    static let maximumDimension = 32_768
    static let maximumPixels = 32 * 1_024 * 1_024
    static let downscaleDimension = 4_096

    static func preparedDimensions(
        width: Int,
        height: Int
    ) throws -> ImageGridReferenceDimensions {
        guard width > 0, height > 0 else {
            throw ImageGridReferencePreparationError.unsafeDimensions
        }
        guard width <= maximumDimension, height <= maximumDimension else {
            throw ImageGridReferencePreparationError.unsafeDimensions
        }
        let pixels = Int64(width) * Int64(height)
        guard pixels <= Int64(maximumPixels) else {
            throw ImageGridReferencePreparationError.unsafeDimensions
        }
        let longest = max(width, height)
        guard longest > downscaleDimension else {
            return ImageGridReferenceDimensions(width: width, height: height)
        }
        let scale = Double(downscaleDimension) / Double(longest)
        return ImageGridReferenceDimensions(
            width: max(1, Int((Double(width) * scale).rounded())),
            height: max(1, Int((Double(height) * scale).rounded()))
        )
    }
}

private enum ImageGridReferenceFormat {
    case png
    case jpeg
    case webP

    var type: UTType {
        switch self {
        case .png: .png
        case .jpeg: .jpeg
        case .webP: .webP
        }
    }

    var fileExtension: String {
        switch self {
        case .png: "png"
        case .jpeg: "jpg"
        case .webP: "webp"
        }
    }

    var encodingProperties: CFDictionary? {
        switch self {
        case .png:
            nil
        case .jpeg, .webP:
            [kCGImageDestinationLossyCompressionQuality: 0.9] as CFDictionary
        }
    }

    init?(sourceType: CFString, allowsTIFFSource: Bool) {
        guard let type = UTType(sourceType as String) else { return nil }
        if type.conforms(to: .png) {
            self = .png
        } else if type.conforms(to: .jpeg) {
            self = .jpeg
        } else if type.conforms(to: .webP) {
            self = .webP
        } else if allowsTIFFSource, type.conforms(to: .tiff) {
            self = .png
        } else {
            return nil
        }
    }
}

struct ImageGridReferenceCandidate: Sendable {
    let url: URL
    let ownsTemporaryFile: Bool
    let allowsTIFFSource: Bool

    init(url: URL, ownsTemporaryFile: Bool, allowsTIFFSource: Bool = false) {
        self.url = url
        self.ownsTemporaryFile = ownsTemporaryFile
        self.allowsTIFFSource = allowsTIFFSource
    }

    func removeOwnedTemporaryFile() {
        guard ownsTemporaryFile else { return }
        try? FileManager.default.removeItem(at: url)
    }
}

struct ImageGridReference: Equatable, Sendable {
    static let maximumBytes: Int64 = 100 * 1_024 * 1_024
    static let supportedExtensions = Set(["png", "jpg", "jpeg", "webp"])

    let url: URL
    let size: Int64
    let pixelWidth: Int
    let pixelHeight: Int
    let ownsTemporaryFile: Bool

    static func prepare(candidate: ImageGridReferenceCandidate) throws -> Self {
        defer {
            candidate.removeOwnedTemporaryFile()
        }
        return try prepare(url: candidate.url, allowsTIFFSource: candidate.allowsTIFFSource)
    }

    static func prepare(url: URL, allowsTIFFSource: Bool = false) throws -> Self {
        do {
            return try prepareValidated(url: url, allowsTIFFSource: allowsTIFFSource)
        } catch let error as ImageGridReferencePreparationError {
            throw error
        } catch {
            throw ImageGridReferencePreparationError.preparationFailed
        }
    }

    private static func prepareValidated(
        url: URL,
        allowsTIFFSource: Bool
    ) throws -> Self {
        guard url.isFileURL else {
            throw ImageGridReferencePreparationError.unsupportedType
        }
        let standardized = url.standardizedFileURL.resolvingSymlinksInPath()
        let pathExtension = standardized.pathExtension.lowercased()
        guard supportedExtensions.contains(pathExtension)
            || (allowsTIFFSource && ["tif", "tiff"].contains(pathExtension))
        else {
            throw ImageGridReferencePreparationError.unsupportedType
        }
        let values = try standardized.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
        guard values.isRegularFile == true else {
            throw ImageGridReferencePreparationError.preparationFailed
        }
        let size = Int64(values.fileSize ?? 0)
        guard size > 0 else {
            throw ImageGridReferencePreparationError.decodeFailed
        }
        guard size <= maximumBytes else {
            throw ImageGridReferencePreparationError.tooLarge
        }

        let sourceOptions = [
            kCGImageSourceShouldCache: false,
            kCGImageSourceShouldCacheImmediately: false,
        ] as CFDictionary
        guard let source = CGImageSourceCreateWithURL(standardized as CFURL, sourceOptions),
              CGImageSourceGetCount(source) > 0,
              CGImageSourceGetStatusAtIndex(source, 0) == .statusComplete,
              let sourceType = CGImageSourceGetType(source)
        else {
            throw ImageGridReferencePreparationError.decodeFailed
        }
        guard let format = ImageGridReferenceFormat(
            sourceType: sourceType,
            allowsTIFFSource: allowsTIFFSource
        ) else {
            throw ImageGridReferencePreparationError.unsupportedType
        }
        guard let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, sourceOptions)
                as? [CFString: Any],
              let width = (properties[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue,
              let height = (properties[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue
        else {
            throw ImageGridReferencePreparationError.decodeFailed
        }
        let dimensions = try ImageGridReferencePolicy.preparedDimensions(
            width: width,
            height: height
        )
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-image-grid-native/references", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let destinationURL = directory.appendingPathComponent(
            "reference-\(UUID().uuidString).\(format.fileExtension)"
        )

        do {
            if dimensions.width == width, dimensions.height == height {
                try FileManager.default.copyItem(at: standardized, to: destinationURL)
            } else {
                let thumbnailOptions = [
                    kCGImageSourceCreateThumbnailFromImageAlways: true,
                    kCGImageSourceCreateThumbnailWithTransform: true,
                    kCGImageSourceThumbnailMaxPixelSize: ImageGridReferencePolicy.downscaleDimension,
                    kCGImageSourceShouldCacheImmediately: true,
                ] as CFDictionary
                guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, thumbnailOptions),
                      let destination = CGImageDestinationCreateWithURL(
                          destinationURL as CFURL,
                          format.type.identifier as CFString,
                          1,
                          nil
                )
                else {
                    throw ImageGridReferencePreparationError.preparationFailed
                }
                CGImageDestinationAddImage(destination, image, format.encodingProperties)
                guard CGImageDestinationFinalize(destination) else {
                    throw ImageGridReferencePreparationError.preparationFailed
                }
            }

            let outputValues = try destinationURL.resourceValues(
                forKeys: [.isRegularFileKey, .fileSizeKey]
            )
            let outputSize = Int64(outputValues.fileSize ?? 0)
            guard outputValues.isRegularFile == true, outputSize > 0 else {
                throw ImageGridReferencePreparationError.preparationFailed
            }
            guard outputSize <= maximumBytes else {
                throw ImageGridReferencePreparationError.tooLarge
            }
            return Self(
                url: destinationURL,
                size: outputSize,
                pixelWidth: dimensions.width,
                pixelHeight: dimensions.height,
                ownsTemporaryFile: true
            )
        } catch {
            try? FileManager.default.removeItem(at: destinationURL)
            throw error
        }
    }

    func removeOwnedTemporaryFile() {
        guard ownsTemporaryFile else { return }
        try? FileManager.default.removeItem(at: url)
    }
}

@MainActor
enum NativeReferencePasteboard {
    static func candidate() throws -> ImageGridReferenceCandidate? {
        let pasteboard = NSPasteboard.general
        let options: [NSPasteboard.ReadingOptionKey: Any] = [.urlReadingFileURLsOnly: true]
        if let urls = pasteboard.readObjects(
            forClasses: [NSURL.self],
            options: options
        ) as? [URL], let first = urls.first {
            return ImageGridReferenceCandidate(url: first, ownsTemporaryFile: false)
        }

        let data: Data?
        let fileExtension: String
        let allowsTIFFSource: Bool
        if let png = pasteboard.data(forType: .png) {
            data = png
            fileExtension = "png"
            allowsTIFFSource = false
        } else if let tiff = pasteboard.data(forType: .tiff) {
            data = tiff
            fileExtension = "tiff"
            allowsTIFFSource = true
        } else {
            data = nil
            fileExtension = "png"
            allowsTIFFSource = false
        }
        guard let data else { return nil }
        guard Int64(data.count) <= ImageGridReference.maximumBytes else {
            throw ImageGridReferencePreparationError.tooLarge
        }
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-image-grid-native", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent(
            "pasted-\(UUID().uuidString).\(fileExtension)"
        )
        try data.write(to: url, options: .atomic)
        return ImageGridReferenceCandidate(
            url: url,
            ownsTemporaryFile: true,
            allowsTIFFSource: allowsTIFFSource
        )
    }

    static func candidate(from providers: [NSItemProvider]) async throws
        -> ImageGridReferenceCandidate?
    {
        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                let item = try await provider.loadItem(
                    forTypeIdentifier: UTType.fileURL.identifier,
                    options: nil
                )
                if let url = item as? URL {
                    return ImageGridReferenceCandidate(url: url, ownsTemporaryFile: false)
                }
                if let data = item as? Data,
                   let url = URL(dataRepresentation: data, relativeTo: nil)
                {
                    return ImageGridReferenceCandidate(url: url, ownsTemporaryFile: false)
                }
            }
            for type in [UTType.png, .jpeg, .webP] {
                guard provider.hasItemConformingToTypeIdentifier(type.identifier) else {
                    continue
                }
                guard let data = try await loadData(
                    from: provider,
                    typeIdentifier: type.identifier
                ) else {
                    continue
                }
                guard Int64(data.count) <= ImageGridReference.maximumBytes else {
                    throw ImageGridReferencePreparationError.tooLarge
                }
                let directory = FileManager.default.temporaryDirectory
                    .appendingPathComponent("codex-image-grid-native/incoming", isDirectory: true)
                try FileManager.default.createDirectory(
                    at: directory,
                    withIntermediateDirectories: true
                )
                let url = directory.appendingPathComponent(
                    "pasted-\(UUID().uuidString).\(type.preferredFilenameExtension ?? "png")"
                )
                try data.write(to: url, options: .atomic)
                return ImageGridReferenceCandidate(url: url, ownsTemporaryFile: true)
            }
        }
        return nil
    }

    private static func loadData(
        from provider: NSItemProvider,
        typeIdentifier: String
    ) async throws -> Data? {
        try await withCheckedThrowingContinuation { continuation in
            _ = provider.loadDataRepresentation(
                forTypeIdentifier: typeIdentifier
            ) { data, error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: data)
                }
            }
        }
    }
}
