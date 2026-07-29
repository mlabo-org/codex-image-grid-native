import AppKit
import Combine
import Foundation
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

private struct ImageGridHealthResponse: Decodable {
    let ok: Bool
    let app: String?
    let generatedDir: String?
    let appServerImageReady: Bool?
    let appServerImageDiagnostics: ImageGridDiagnostics?
}

private struct ImageGridDiagnostics: Decodable {
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

struct ImageGridAPIClient: Sendable {
    static let defaultBaseURL = URL(string: "http://127.0.0.1:4322")!

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
        guard response.ok, response.app == nil || response.app == "codex-image-grid-native" else {
            throw ImageGridAPIError(message: "The local Image Grid server identity is invalid.")
        }
        let diagnostics = response.appServerImageDiagnostics
        let ready = response.appServerImageReady == true || diagnostics?.ready == true
            || diagnostics?.status == "ready"
        let failed = ["error", "failed", "stopped", "exited"].contains(diagnostics?.status ?? "")
        let directory = response.generatedDir.map { URL(fileURLWithPath: $0) }
        return (failed ? .error : (ready ? .ready : .idle), directory)
    }

    func preflight() async throws {
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
        try await send(
            path: batch ? "/api/run-batch" : "/api/run",
            method: "POST",
            body: request
        )
    }

    func runs() async throws -> [ImageGridRunEnvelope] {
        let response: ImageGridRunListResponse = try await send(path: "/api/runs")
        return response.data
    }

    func analyze(referenceImagePath: String) async throws -> String {
        let response: ImageGridAnalysisResponse = try await send(
            path: "/api/analyze-reference",
            method: "POST",
            body: ImageGridAnalysisRequest(referenceImagePath: referenceImagePath)
        )
        guard let premise = response.premise?.trimmingCharacters(in: .whitespacesAndNewlines),
              !premise.isEmpty
        else {
            throw ImageGridAPIError(message: "Reference analysis returned no premise.")
        }
        return premise
    }

    func consumeEvents(
        onEvent: @escaping @Sendable (ImageGridSSEEvent) async -> Void
    ) async throws {
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
        var request = URLRequest(url: endpoint(path))
        request.httpMethod = method
        request.timeoutInterval = 20
        request.cachePolicy = .reloadIgnoringLocalCacheData
        if let bodyData {
            request.httpBody = bodyData
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
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
            terminal = Array(terminal.prefix(max(0, min(96, completedLimit))))
        }
        return active + terminal
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

@MainActor
final class ImageGridStore: ObservableObject {
    @Published private(set) var jobs: [String: ImageGridJob] = [:]
    @Published private(set) var runtimeState = RuntimeConnectionState.disconnected
    @Published private(set) var generatedDirectory: URL?
    @Published private(set) var isSubmitting = false
    @Published private(set) var isAnalyzing = false
    @Published var generationMessage: String?
    @Published var referenceAnalysisMessage: String?

    let client: ImageGridAPIClient
    private var lifecycleTask: Task<Void, Never>?
    private var clearedBefore: Int64 = 0
    private var clearedJobIDs: Set<String> = []

    init(client: ImageGridAPIClient = ImageGridAPIClient()) {
        self.client = client
    }

    func start() {
        guard lifecycleTask == nil else { return }
        lifecycleTask = Task { [weak self] in
            guard let self else { return }
            await refreshHealth()
            await hydrateRuns()
            await consumeEventsWithBoundedRecovery()
            lifecycleTask = nil
        }
    }

    func stop() {
        lifecycleTask?.cancel()
        lifecycleTask = nil
    }

    func refreshHealth() async {
        runtimeState = .starting
        do {
            let health = try await client.health()
            runtimeState = health.state
            generatedDirectory = health.generatedDirectory
        } catch {
            runtimeState = .disconnected
            generationMessage = error.localizedDescription
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

    func visibleJobs(resultLimit: ResultLimit, showFailed: Bool) -> [ImageGridJob] {
        ImageGridJobSelection.visible(
            jobs: jobs.values,
            completedLimit: resultLimit.completedLimit,
            showFailed: showFailed
        )
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
        for job in terminal {
            clearedJobIDs.insert(job.id)
            jobs.removeValue(forKey: job.id)
        }
        if clearedJobIDs.count > 512 {
            clearedJobIDs = Set(clearedJobIDs.suffix(512))
        }
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

    private func consumeEventsWithBoundedRecovery() async {
        var failures = 0
        while !Task.isCancelled, failures < 5 {
            do {
                try await client.consumeEvents { [weak self] event in
                    await self?.receive(event)
                }
            } catch is CancellationError {
                return
            } catch {
                failures += 1
                runtimeState = .disconnected
                if failures < 5 {
                    let delay = Double(min(16, 1 << (failures - 1)))
                    try? await Task.sleep(for: .seconds(delay))
                }
            }
        }
        if !Task.isCancelled {
            generationMessage = "Live result updates disconnected. Generate again to retry."
        }
    }

    private func receive(_ event: ImageGridSSEEvent) {
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
        default:
            break
        }
    }

    private func merge(_ incoming: [ImageGridJob]) {
        for job in incoming {
            let existing = jobs[job.id]
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
            jobs[job.id] = job
        }
    }
}

struct ImageGridReference: Equatable, Sendable {
    static let maximumBytes: Int64 = 100 * 1_024 * 1_024
    static let supportedExtensions = Set(["png", "jpg", "jpeg", "webp"])

    let url: URL
    let size: Int64
    let ownsTemporaryFile: Bool

    static func validate(url: URL, ownsTemporaryFile: Bool = false) throws -> Self {
        guard url.isFileURL else {
            throw ImageGridAPIError(message: "Choose a local PNG, JPEG, or WebP image.")
        }
        let standardized = url.standardizedFileURL.resolvingSymlinksInPath()
        guard supportedExtensions.contains(standardized.pathExtension.lowercased()) else {
            throw ImageGridAPIError(message: "Choose a PNG, JPEG, or WebP image.")
        }
        let values = try standardized.resourceValues(
            forKeys: [.isRegularFileKey, .fileSizeKey, .contentTypeKey]
        )
        guard values.isRegularFile == true else {
            throw ImageGridAPIError(message: "The reference image must be a regular file.")
        }
        if let contentType = values.contentType {
            let allowed = [UTType.png, .jpeg, .webP].contains { contentType.conforms(to: $0) }
            guard allowed else {
                throw ImageGridAPIError(message: "Choose a PNG, JPEG, or WebP image.")
            }
        }
        let size = Int64(values.fileSize ?? 0)
        guard size > 0, size <= maximumBytes else {
            throw ImageGridAPIError(message: "The reference image must be 100 MiB or smaller.")
        }
        return Self(url: standardized, size: size, ownsTemporaryFile: ownsTemporaryFile)
    }

    func removeOwnedTemporaryFile() {
        guard ownsTemporaryFile else { return }
        try? FileManager.default.removeItem(at: url)
    }
}

@MainActor
enum NativeReferencePasteboard {
    static func reference() throws -> ImageGridReference? {
        let pasteboard = NSPasteboard.general
        let options: [NSPasteboard.ReadingOptionKey: Any] = [.urlReadingFileURLsOnly: true]
        if let urls = pasteboard.readObjects(
            forClasses: [NSURL.self],
            options: options
        ) as? [URL], let first = urls.first {
            return try ImageGridReference.validate(url: first)
        }

        let data: Data?
        if let png = pasteboard.data(forType: .png) {
            data = png
        } else if let tiff = pasteboard.data(forType: .tiff),
                  let representation = NSBitmapImageRep(data: tiff)
        {
            data = representation.representation(using: .png, properties: [:])
        } else {
            data = nil
        }
        guard let data else { return nil }
        guard Int64(data.count) <= ImageGridReference.maximumBytes else {
            throw ImageGridAPIError(message: "The pasted image must be 100 MiB or smaller.")
        }
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-image-grid-native", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent("pasted-\(UUID().uuidString).png")
        try data.write(to: url, options: .atomic)
        do {
            return try ImageGridReference.validate(url: url, ownsTemporaryFile: true)
        } catch {
            try? FileManager.default.removeItem(at: url)
            throw error
        }
    }
}
