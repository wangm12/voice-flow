import XCTest

@testable import VoiceFlowMLXSidecarCore

final class ChunkBoundaryTextJoinerTests: XCTestCase {
    func testEnglishWordsReceiveBoundarySpace() {
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["hello", "world"]), "hello world")
    }

    func testAdjacentCJKTextAndPunctuationStayJoined() {
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["你好", "世界"]), "你好世界")
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["你好，", "世界"]), "你好，世界")
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["你好", "，", "世界。"]), "你好，世界。")
    }

    func testMixedLanguageBoundariesReceiveSpace() {
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["你好", "world", "再见"]), "你好 world 再见")
    }

    func testWesternPunctuationStaysAttachedAndAllowsFollowingWordSpace() {
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["hello", ",", "world", "!"]), "hello, world!")
    }

    func testCJKQuotationMarksStayAttached() {
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["「", "你好", "」"]), "「你好」")
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["你好", "「世界」"]), "你好「世界」")
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["“你好”", "世界"]), "“你好”世界")
        XCTAssertEqual(ChunkBoundaryTextJoiner.join(["“hello”", "world"]), "“hello” world")
    }
}
