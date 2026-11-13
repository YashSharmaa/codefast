import Foundation
@testable import Networking
@_exported import NIOCore
import struct NIO.ByteBuffer
import Darwin.C

/// A shape that can be drawn.
public protocol Shape: AnyObject, Drawable {
    associatedtype Unit: Measure
    var area: Double { get }
    var label: String { get set }
    func draw(in context: Context, scale: Int) async throws -> Bool
    static func make() -> Self
    init(size: Int)
    subscript(index: Int) -> Int { get }
}

@MainActor
open class Base<T: Equatable>: NSObject, Shape where T: Hashable {
    private var count: Int = 0
    fileprivate let name: String
    private(set) var visible = true
    lazy var cache = Cache()
    @Published public var value: Int = 1 {
        willSet { log(newValue) }
        didSet(old) { update(old) }
    }
    var computed: Int {
        get { return count * 2 }
        set { count = newValue }
    }
    var readOnly: Int { count + 1 }
    weak var delegate: Delegate?
    static let shared = Base(name: "shared")
    var onTap: ((Int, String) -> Void)?
    var items: [Item] = []
    var lookup: [String: Item] = [:]

    public init(name: String = "x", _ extra: Int...) {
        self.name = name
        super.init()
    }

    convenience init?(other: Base) {
        self.init(name: other.name)
    }

    deinit {
        cleanup()
    }

    public func draw(in context: Context, scale: Int = 1) async throws -> Bool {
        return true
    }

    static func make() -> Self { fatalError() }
    class func classMethod() {}
    subscript(index: Int) -> Int {
        get { index }
        set { count = newValue }
    }
    private func helper(_ a: Int, _ b: Int = 0) {}

    func outer() {
        func inner(_ x: Int) -> Int { x }
        struct Local { var v: Int }
        _ = inner(1)
    }

    static func == (lhs: Base, rhs: Base) -> Bool { true }
}

extension Base: CustomStringConvertible, Printable where T == Int {
    public var description: String { "" }
    func extra() {}
    struct Nested {
        var q: Int
        enum Mode { case on, off }
    }
}

extension Outer.Inner {
    func deep() {}
}

extension String {
    func shout() -> String { self }
}

public enum Kind: String, Codable {
    case alpha, beta
    case gamma = "g"
}

indirect enum Tree {
    case leaf
    case node(Tree, value: Int, Tree)
    func depth() -> Int { 0 }
}

struct Point: Hashable {
    var x, y: Double
    let z = 1
}

actor Counter {
    var n = 0
    func increment() async { n += 1 }
}

typealias Handler = (Int) -> Void
typealias Points = [Point]
public let globalConstant = 42
var globalVariable: [String] = []
let (firstPart, secondPart) = (1, 2)

func freeFunction<U>(_ u: U, label l: Int = 1, rest: Int...) -> U { u }

infix operator <~> : AdditionPrecedence

func <~> (lhs: Int, rhs: Int) -> Int { lhs + rhs }

@propertyWrapper
struct Clamped {
    var wrappedValue: Int
}

final class ViewController: UIViewController, UITableViewDelegate, ModelDelegate {
    @IBOutlet weak var table: UITableView!
    @Clamped var level: Int = 0

    override func viewDidLoad() {
        super.viewDidLoad()
    }
}
