import Testing
import Foundation
@testable import CodexImageGridNative

@Test func frozenUiChoicesAndDefaults() {
    #expect(ImageGridContract.counts == [1, 2, 3, 4, 6])
    #expect(ImageMood.allCases.map(\.rawValue) == [
        "warm-mascot",
        "clean-thumbnail",
        "editorial-soft",
        "cinematic",
        "minimal-product",
    ])
    #expect(AspectRatio.allCases.map(\.rawValue) == ["16:9", "4:3", "1:1", "3:4", "9:16"])
    #expect(ResultLimit.allCases.map(\.rawValue) == ["6", "12", "24", "48", "96", "all"])
    #expect(ImageGridContract.defaultBatchPrompts.count == 3)
}

@Test func batchJobLimitMatchesTheFrozenRenderer() {
    let fourPrompts = Array(repeating: "prompt", count: 4)
    let fivePrompts = Array(repeating: "prompt", count: 5)

    #expect(ImageGridContract.batchJobCount(prompts: fourPrompts, count: 6) == 24)
    #expect(ImageGridContract.batchIsValid(prompts: fourPrompts, count: 6))
    #expect(!ImageGridContract.batchIsValid(prompts: fivePrompts, count: 6))
}

@Test func generationRequestUsesNativeReferencePathContract() throws {
    let request = ImageGridGenerationRequest(
        prompt: "first",
        prompts: ["first", "second"],
        referencePremise: "same character",
        mood: "warm-mascot",
        engine: "app-server-image",
        count: 2,
        aspectRatio: "16:9",
        referenceImagePath: "/tmp/reference.png"
    )

    let object = try #require(
        JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? [String: Any]
    )
    #expect(object["prompt"] as? String == "first")
    #expect(object["prompts"] as? [String] == ["first", "second"])
    #expect(object["referenceImagePath"] as? String == "/tmp/reference.png")
    #expect(object["referenceImage"] == nil)
}

@Test func sseParserBuildsNamedMultilineEvent() throws {
    var parser = ImageGridSSEParser()
    #expect(parser.consume(line: "event: job") == nil)
    #expect(parser.consume(line: "data: {\"id\":\"one\",") == nil)
    #expect(parser.consume(line: "data: \"status\":\"running\"}") == nil)
    let parsed = parser.consume(line: "")
    let event = try #require(parsed)

    #expect(event.name == "job")
    #expect(String(data: event.data, encoding: .utf8) == "{\"id\":\"one\",\n\"status\":\"running\"}")
}

@Test func visibleJobsKeepActiveAndApplyCompletedFilters() {
    let jobs = [
        ImageGridJob(
            id: "active",
            status: "running",
            updatedAt: ImageGridTimestamp(milliseconds: 1)
        ),
        ImageGridJob(
            id: "done-new",
            status: "done",
            updatedAt: ImageGridTimestamp(milliseconds: 30)
        ),
        ImageGridJob(
            id: "failed",
            status: "error",
            updatedAt: ImageGridTimestamp(milliseconds: 20)
        ),
        ImageGridJob(
            id: "done-old",
            status: "done",
            updatedAt: ImageGridTimestamp(milliseconds: 10)
        ),
    ]

    let hiddenFailures = ImageGridJobSelection.visible(
        jobs: jobs,
        completedLimit: 1,
        showFailed: false
    )
    #expect(hiddenFailures.map(\.id) == ["active", "done-new"])

    let shownFailures = ImageGridJobSelection.visible(
        jobs: jobs,
        completedLimit: 2,
        showFailed: true
    )
    #expect(shownFailures.map(\.id) == ["active", "failed", "done-new"])
}

@Test func hiddenFailureNoticeCopyIsExplicitAndLocalized() {
    let japanese = ImageGridStrings(language: .japanese)
    let english = ImageGridStrings(language: .english)

    #expect(japanese.generationFailed == "画像生成に失敗しました")
    #expect(japanese.hiddenFailureMessage(count: 2) == "2件の失敗結果は表示設定により非表示です。")
    #expect(japanese.showFailedResults == "失敗した結果を表示")
    #expect(english.generationFailed == "Image generation failed")
    #expect(
        english.hiddenFailureMessage(count: 1)
            == "1 failed result is hidden by your display setting."
    )
    #expect(
        english.hiddenFailureMessage(count: 2)
            == "2 failed results are hidden by your display setting."
    )
    #expect(english.showFailedResults == "Show failed results")
}

@Test func failureNoticeTracksOnlyJobsObservedActiveInThisSession() {
    var tracker = ImageGridFailureNoticeTracker()
    let historicalFailure = ImageGridJob(
        id: "historical-failure",
        status: "error",
        updatedAt: ImageGridTimestamp(milliseconds: 1)
    )
    tracker.observe(previous: nil, incoming: historicalFailure)
    #expect(tracker.unacknowledgedCount == 0)

    let active = ImageGridJob(
        id: "current-job",
        status: "running",
        updatedAt: ImageGridTimestamp(milliseconds: 2)
    )
    tracker.observe(previous: nil, incoming: active)
    #expect(tracker.unacknowledgedCount == 0)

    let currentFailure = ImageGridJob(
        id: "current-job",
        status: "error",
        updatedAt: ImageGridTimestamp(milliseconds: 3)
    )
    tracker.observe(previous: active, incoming: currentFailure)
    #expect(tracker.unacknowledgedCount == 1)

    tracker.acknowledgeFailures()
    #expect(tracker.unacknowledgedCount == 0)

    tracker.observe(previous: currentFailure, incoming: currentFailure)
    #expect(tracker.unacknowledgedCount == 0)
}

@Test func activeImageLoadFailureRemainsAProgressState() {
    let active = ImageGridJob(id: "active", status: "running")
    let failed = ImageGridJob(id: "failed", status: "error")
    let unavailable = ImageGridJob(id: "unavailable", status: "done")

    #expect(ResultCardImageLoadFailurePresentation.resolve(for: active) == .progress)
    #expect(
        ResultCardImageLoadFailurePresentation.resolve(for: failed)
            == .generationFailure
    )
    #expect(
        ResultCardImageLoadFailurePresentation.resolve(for: unavailable)
            == .imageUnavailable
    )
}

@Test func completedJobGetsANewImageRequestIdentityForAutomaticRetry() throws {
    let imageURL = try #require(URL(string: "http://127.0.0.1:4322/generated/result.png"))
    let activeIdentity = ResultCardImageRequestIdentity(
        jobID: "job",
        jobStatus: "running",
        imageURL: imageURL
    )
    let completedIdentity = ResultCardImageRequestIdentity(
        jobID: "job",
        jobStatus: "done",
        imageURL: imageURL
    )

    #expect(activeIdentity != completedIdentity)
}

@Test func responsiveGridUsesAvailableWidthWithoutAFixedColumnCap() {
    let grid = ResponsiveResultGrid(minimumColumnWidth: 512, spacing: 16)

    #expect(grid.columnCount(for: 480, itemCount: 20) == 1)
    #expect(grid.columnCount(for: 1_600, itemCount: 20) == 3)
    #expect(grid.columnCount(for: 3_840, itemCount: 20) == 7)
    #expect(grid.columnCount(for: 3_840, itemCount: 2) == 2)
    #expect(grid.gridItems(for: 3_840, itemCount: 20).count == 7)
    #expect(grid.gridItems(for: 3_840, itemCount: 2).count == 2)
}

@Test func referenceContractMatchesNativeServerLimitAndFormats() {
    #expect(ImageGridReference.maximumBytes == 100 * 1_024 * 1_024)
    #expect(ImageGridReference.supportedExtensions == ["png", "jpg", "jpeg", "webp"])
}

@Test func referenceDimensionPolicyRejectsUnsafeImagesAndDownscales() throws {
    #expect(throws: ImageGridReferencePreparationError.unsafeDimensions) {
        try ImageGridReferencePolicy.preparedDimensions(width: 32_769, height: 1)
    }
    #expect(throws: ImageGridReferencePreparationError.unsafeDimensions) {
        try ImageGridReferencePolicy.preparedDimensions(width: 6_000, height: 6_000)
    }

    let landscape = try ImageGridReferencePolicy.preparedDimensions(
        width: 8_000,
        height: 4_000
    )
    #expect(landscape == ImageGridReferenceDimensions(width: 4_096, height: 2_048))

    let portrait = try ImageGridReferencePolicy.preparedDimensions(
        width: 2_000,
        height: 8_000
    )
    #expect(portrait == ImageGridReferenceDimensions(width: 1_024, height: 4_096))
}

@Test func malformedReferenceHeaderIsRejectedBeforePreparation() throws {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("invalid-reference-\(UUID().uuidString).png")
    try Data([0x89, 0x50, 0x4e, 0x47]).write(to: url)
    defer {
        try? FileManager.default.removeItem(at: url)
    }

    #expect(throws: ImageGridReferencePreparationError.decodeFailed) {
        try ImageGridReference.prepare(url: url)
    }
}

@Test func referencePreparationStatusesAndErrorsMatchFrozenCopy() {
    let japanese = ImageGridStrings(language: .japanese)
    let english = ImageGridStrings(language: .english)

    #expect(japanese.referenceReady == "参照画像を追加しました。")
    #expect(english.referenceReady == "Reference image added.")
    #expect(japanese.referencePreparing == "参照画像を準備しています...")
    #expect(english.referencePreparing == "Preparing reference image...")
    #expect(japanese.referenceAnalyzing == "参照画像を解析中...")
    #expect(english.referenceAnalyzing == "Analyzing reference image...")

    #expect(
        japanese.referencePreparationError(.unsupportedType)
            == "PNG、JPEG、WebP画像を選択してください。"
    )
    #expect(
        english.referencePreparationError(.unsupportedType)
            == "Choose a PNG, JPEG, or WebP image."
    )
    #expect(
        japanese.referencePreparationError(.tooLarge)
            == "参照画像は100MB以下にしてください。"
    )
    #expect(
        english.referencePreparationError(.tooLarge)
            == "The reference image must be 100 MB or smaller."
    )
    #expect(
        japanese.referencePreparationError(.unsafeDimensions)
            == "参照画像の寸法が大きすぎるため、安全に処理できません。"
    )
    #expect(
        english.referencePreparationError(.unsafeDimensions)
            == "The reference image dimensions are too large to process safely."
    )
    #expect(
        japanese.referencePreparationError(.decodeFailed)
            == "参照画像を読み取れませんでした。"
    )
    #expect(
        english.referencePreparationError(.decodeFailed)
            == "The reference image could not be decoded."
    )
    #expect(
        japanese.referencePreparationError(.preparationFailed)
            == "参照画像を準備できませんでした。"
    )
    #expect(
        english.referencePreparationError(.preparationFailed)
            == "The reference image could not be prepared."
    )
}

@Test func analysisRequestUsesNativePathAndCoversServerTimeout() throws {
    let client = ImageGridAPIClient(
        baseURL: URL(string: "http://127.0.0.1:4322")!
    )
    let request = try client.analysisRequest(referenceImagePath: "/tmp/reference.webp")
    let body = try #require(request.httpBody)
    let object = try #require(
        JSONSerialization.jsonObject(with: body) as? [String: Any]
    )

    #expect(request.httpMethod == "POST")
    #expect(request.url?.path == "/api/analyze-reference")
    #expect(request.timeoutInterval >= 180)
    #expect(object["referenceImagePath"] as? String == "/tmp/reference.webp")
    #expect(object["referenceImage"] == nil)
}
