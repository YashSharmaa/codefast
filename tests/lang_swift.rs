#![cfg(feature = "lang-swift")]
//! End-to-end checks of Swift linking: a small SwiftPM package is written to a temporary
//! directory, indexed, and asked through the command line, the way a harness would.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_codefast");

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Project {
        let root = std::env::temp_dir().join(format!("codefast-swift-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in files {
            let p = root.join("src").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
        }
        Project { root }
    }

    fn ask(&self, args: &[&str]) -> String {
        let out = Command::new(BIN)
            .arg("--root")
            .arg(self.root.join("src"))
            .args(args)
            .env("CODEFAST_INDEX_DIR", self.root.join("index"))
            .output()
            .expect("run codefast");
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn has(text: &str, needle: &str) {
    assert!(text.contains(needle), "expected `{needle}` in:\n{text}");
}

fn lacks(text: &str, needle: &str) {
    assert!(!text.contains(needle), "did not expect `{needle}` in:\n{text}");
}

const MANIFEST: &str = r#"// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Shop",
    targets: [
        .target(name: "Core", path: "Sources/Core"),
        .target(name: "App", dependencies: ["Core"]),
        .testTarget(name: "AppTests", dependencies: ["App"]),
    ]
)
"#;

/// A package with two library modules and a test module.
fn shop(name: &str) -> Project {
    Project::new(
        name,
        &[
            ("Package.swift", MANIFEST),
            (
                "Sources/Core/Shapes.swift",
                r#"public protocol Shape {
    func area() -> Double
}

public struct Circle: Shape {
    public var radius: Double

    public init(radius: Double) {
        self.radius = radius
    }

    public init(diameter: Double, scale: Double) {
        self.radius = diameter / 2 * scale
    }

    public func area() -> Double {
        return 3.14 * radius * radius
    }
}

public struct Square: Shape {
    public var side: Double

    public func area() -> Double {
        side * side
    }
}

public enum Direction {
    case north, south
}

public final class Registry {
    public static func shared() -> Registry {
        Registry()
    }

    public var onChange: ((Int) -> Void)?
    public var count = 0

    public init() {}

    public func register(_ s: Shape) {
        count += 1
    }

    public struct Entry {
        public var name: String
    }
}

open class Job {
    open func stop() {}
}

public final class Download: Job {
    public func stop(force: Bool) {}
}
"#,
            ),
            (
                "Sources/Core/Geometry.swift",
                r#"extension Circle {
    public func diameter() -> Double {
        radius * 2
    }
}

public func total(_ shapes: [Shape]) -> Double {
    var sum = 0.0
    for s in shapes {
        sum += s.area()
    }
    return sum
}

func clamp(_ v: Double) -> Double { v }
"#,
            ),
            (
                "Sources/App/Controller.swift",
                r#"import Core

func handle(_ v: Int) {}
func transform(_ x: Int) -> Int { x }

final class Controller {
    let registry = Registry.shared()
    var heading: Direction = .north

    func run() {
        let c = Circle(radius: 2)
        _ = c.area()
        _ = c.diameter()
        registry.onChange = handle
        _ = [1, 2].map(transform)
        registry.register(c)
        heading = .south
        print(registry.count)
        let big = Circle(diameter: 4, scale: 1)
        _ = big
        log()
        _ = Registry.Entry(name: "x").label()
    }
}
"#,
            ),
            (
                "Sources/App/Helpers.swift",
                r#"import Core

extension Registry.Entry {
    func label() -> String { name }
}

func log() {}

func clamp(_ v: Double) -> Double { v + 1 }

func measure(_ s: Square) -> Double {
    clamp(s.area())
}

func halt(_ d: Download) {
    d.stop()
    d.stop(force: true)
}
"#,
            ),
            (
                "Tests/AppTests/CircleTests.swift",
                r#"import XCTest
@testable import Core

final class CircleTests: XCTestCase {
    func testArea() {
        XCTAssertEqual(Circle(radius: 1).area(), 3.14)
    }
}
"#,
            ),
        ],
    )
}

#[test]
fn definitions_are_found_and_shown() {
    let p = shop("definitions_are_found_and_shown");
    let find = p.ask(&["find", "Circle"]);
    has(&find, "Sources/Core/Shapes.swift:5 public struct Circle: Shape");
    has(&find, "Sources/Core/Geometry.swift:1 extension Circle");
    let show = p.ask(&["show", "Circle.diameter"]);
    has(&show, "public func diameter() -> Double");
    let kinds = p.ask(&["find", "Direction", "-k", "enum"]);
    has(&kinds, "public enum Direction");
}

#[test]
fn callers_across_files_and_modules_through_a_typed_receiver() {
    let p = shop("callers_across_files_and_modules_through_a_typed_receiver");
    let callers = p.ask(&["callers", "Circle.area"]);
    has(&callers, "Sources/App/Controller.swift");
    has(&callers, "12 run: _ = c.area()");
    // A call through the protocol may land here, and is listed as such.
    has(&callers, "Sources/Core/Geometry.swift");
    // The other implementation's own caller is not one of this method's.
    lacks(&callers, "measure");
}

#[test]
fn protocol_dispatch_lists_the_conforming_implementations() {
    let p = shop("protocol_dispatch_lists_the_conforming_implementations");
    let callees = p.ask(&["callees", "total"]);
    has(&callees, "Shape.area");
    has(&callees, "Circle.area");
    has(&callees, "Square.area");
    let callers = p.ask(&["callers", "Shape.area"]);
    has(&callers, "10 total: sum += s.area()");
}

#[test]
fn a_method_added_in_an_extension_in_another_file() {
    let p = shop("a_method_added_in_an_extension_in_another_file");
    let callers = p.ask(&["callers", "Circle.diameter"]);
    has(&callers, "13 run: _ = c.diameter()");
    has(&p.ask(&["callees", "run"]), "Circle.diameter Sources/Core/Geometry.swift:2");
    // An extension of a nested type of another module.
    has(&p.ask(&["callers", "Registry.Entry.label"]), "22 run: _ = Registry.Entry(name: \"x\").label()");
}

#[test]
fn static_methods_and_initialisers() {
    let p = shop("static_methods_and_initialisers");
    has(&p.ask(&["callers", "Registry.shared"]), "7 registry: let registry = Registry.shared()");
    // `T(..)` runs an initialiser: the one whose parameters fit the arguments.
    let init = p.ask(&["callers", "Circle.init"]);
    has(&init, "11 run: let c = Circle(radius: 2)");
    has(&init, "19 run: let big = Circle(diameter: 4, scale: 1)");
    // A call whose arguments no overload of the method accepts is a call of another one.
    let stop = p.ask(&["callers", "Job.stop"]);
    has(&stop, "16 halt: d.stop()");
    lacks(&stop, "force: true");
    has(&p.ask(&["callers", "Download.stop"]), "17 halt: d.stop(force: true)");
    has(&p.ask(&["callers", "Circle"]), "11 run: let c = Circle(radius: 2)");
}

#[test]
fn enum_cases_and_properties_used_across_files() {
    let p = shop("enum_cases_and_properties_used_across_files");
    has(&p.ask(&["refs", "Direction.north"]), "8 heading: var heading: Direction = .north");
    has(&p.ask(&["refs", "Direction.south"]), "17 run: heading = .south");
    let count = p.ask(&["refs", "Registry.count"]);
    has(&count, "18 run: print(registry.count)");
    has(&count, "Sources/Core/Shapes.swift");
}

#[test]
fn functions_passed_as_callbacks() {
    let p = shop("functions_passed_as_callbacks");
    let handle = p.ask(&["callers", "handle"]);
    has(&handle, "14 run: registry.onChange = handle");
    has(&handle, "Registry.onChange");
    let transform = p.ask(&["callers", "transform"]);
    has(&transform, "to map()");
    has(&p.ask(&["callees", "run"]), "transform Sources/App/Controller.swift:4 [callback passed to map()]");
}

#[test]
fn module_scope_decides_which_function_a_bare_call_reaches() {
    let p = shop("module_scope_decides_which_function_a_bare_call_reaches");
    // `log()` inside a type with no member `log` is the function of the module.
    has(&p.ask(&["callers", "log"]), "21 run: log()");
    // Each module has its own `clamp`.
    let app = p.ask(&["callers", "Sources/App/Helpers.swift:9"]);
    has(&app, "12 measure: clamp(s.area())");
    let core = p.ask(&["callers", "Sources/Core/Geometry.swift:14"]);
    lacks(&core, "measure");
}

#[test]
fn test_methods_are_test_code() {
    let p = shop("test_methods_are_test_code");
    // Without --tests the call from the test is named apart, without its line.
    let without = p.ask(&["callers", "Circle.area"]);
    has(&without, "1 more in tests");
    has(&without, "Tests/AppTests/CircleTests.swift: testArea:6");
    lacks(&without, "XCTAssertEqual(Circle(radius: 1).area(), 3.14)");
    let with = p.ask(&["callers", "Circle.area", "--tests"]);
    has(&with, "6 testArea: XCTAssertEqual(Circle(radius: 1).area(), 3.14)");
}
