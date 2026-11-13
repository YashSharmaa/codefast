import XCTest
import Testing
@testable import App

class BaseTestCase: XCTestCase {
    func testBase() {}
    func helperMethod(_ x: Int) {}
}

final class SessionTests: BaseTestCase {
    func testRequest() {
        XCTAssertEqual(Session().request(), 1)
    }
    func testWithArgument(_ x: Int) {}
    func notATest() {}
}

struct Plain {
    func testLooksLikeOne() {}
}

@Test func standalone() async throws {
    #expect(compute() == 2)
}

@Suite struct Grouped {
    @Test("named") func inside() {}
}
