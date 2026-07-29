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

@Test func referenceContractMatchesNativeServerLimitAndFormats() {
    #expect(ImageGridReference.maximumBytes == 100 * 1_024 * 1_024)
    #expect(ImageGridReference.supportedExtensions == ["png", "jpg", "jpeg", "webp"])
}
