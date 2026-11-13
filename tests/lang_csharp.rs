#![cfg(feature = "lang-csharp")]
//! End-to-end checks of C# linking: a small project is written to a temporary directory,
//! indexed, and asked through the command line, the way a harness would.

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_codefast");

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Project {
        let root = std::env::temp_dir().join(format!("codefast-csharp-{}-{name}", std::process::id()));
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

const SHAPES: &str = r#"namespace Acme.Core
{
    public interface IShape
    {
        double Area();
    }

    public abstract class Shape : IShape
    {
        public abstract double Area();
        public virtual string Describe() => "shape";
        public int Id { get; set; }
    }

    public class Circle : Shape
    {
        public override double Area() => 3.0;
        public override string Describe() => base.Describe() + "c";
    }

    public class Square : Shape
    {
        public override double Area() => 4.0;
    }
}
"#;

const EXTENSIONS: &str = r#"namespace Acme.Core;

public static class ShapeExtensions
{
    public static double Doubled(this Shape shape) => shape.Area() * 2;
    public static string Shout(this string text) => text + "!";
}
"#;

const EVENTS: &str = r#"namespace Acme.Core
{
    public delegate void Notify(string message);

    public class Bus
    {
        public event Notify Sent;

        public void Send(string message)
        {
            Sent?.Invoke(message);
        }
    }

    public static class MathUtil
    {
        public static int Twice(int x) => x * 2;
    }
}
"#;

const PROGRAM: &str = r#"using Acme.Core;
using static Acme.Core.MathUtil;

namespace Acme.App
{
    public class Program
    {
        public static void Main()
        {
            var c = new Circle();
            c.Area();
            IShape s = c;
            s.Area();
            var d = c.Doubled();
            var loud = "hi".Shout();
            c.Id = 3;
            var i = c.Id;
            var b = new Bus();
            b.Sent += OnSent;
            b.Send("x");
            Twice(2);
            Helper.Run();
        }

        static void OnSent(string message) { }
    }
}
"#;

const HELPER: &str = r#"namespace Acme.App
{
    class Helper
    {
        public static void Run() { }
    }
}
"#;

const TESTS: &str = r#"using Acme.Core;
using Xunit;

namespace Acme.Tests
{
    public class CircleTests
    {
        [Fact]
        public void AreaWorks()
        {
            new Circle().Area();
        }
    }
}
"#;

fn shapes_project(name: &str) -> Project {
    Project::new(
        name,
        &[
            ("Core/Shapes.cs", SHAPES),
            ("Core/Extensions.cs", EXTENSIONS),
            ("Core/Events.cs", EVENTS),
            ("App/Program.cs", PROGRAM),
            ("App/Helper.cs", HELPER),
            ("Acme.Tests/CircleTests.cs", TESTS),
        ],
    )
}

#[test]
fn definitions_are_found_and_shown() {
    let p = shapes_project("defs");
    let find = p.ask(&["find", "Circle"]);
    has(&find, "Core/Shapes.cs:15 public class Circle : Shape");
    let show = p.ask(&["show", "Circle.Area"]);
    has(&show, "public override double Area() => 3.0;");
    let outline = p.ask(&["show", "Core/Events.cs"]);
    has(&outline, "Notify");
    has(&outline, "Sent");
}

#[test]
fn callers_across_files_through_a_typed_receiver() {
    let p = shapes_project("typed");
    let callers = p.ask(&["callers", "Bus.Send"]);
    has(&callers, "all 1 call sites, 1 files");
    has(&callers, "20 Main: b.Send(\"x\");");
    let callees = p.ask(&["callees", "Main"]);
    has(&callees, "Bus.Send Core/Events.cs:9");
    has(&callees, "Circle.Area Core/Shapes.cs:17");
}

#[test]
fn a_call_through_the_interface_reaches_the_implementations() {
    let p = shapes_project("iface");
    let callers = p.ask(&["callers", "Circle.Area"]);
    has(&callers, "11 Main: c.Area();");
    has(&callers, "13 Main: s.Area();");
    // `Square` is never the receiver's type, but the interface call may land there.
    let square = p.ask(&["callers", "Square.Area"]);
    has(&square, "13 Main: s.Area();");
    lacks(&square, "11 Main");
    let callees = p.ask(&["callees", "Main"]);
    has(&callees, "IShape.Area");
}

#[test]
fn base_calls_and_inheritance() {
    let p = shapes_project("base");
    let callers = p.ask(&["callers", "Shape.Describe"]);
    has(&callers, "Describe: public override string Describe() => base.Describe() + \"c\";");
    let refs = p.ask(&["refs", "Shape"]);
    has(&refs, "Core/Shapes.cs");
    has(&refs, "Core/Extensions.cs");
}

#[test]
fn extension_methods_are_members_of_the_extended_type() {
    let p = shapes_project("ext");
    let callers = p.ask(&["callers", "Doubled"]);
    has(&callers, "all 1 call sites, 1 files");
    has(&callers, "14 Main: var d = c.Doubled();");
    // An extension of a type of the platform, called on a literal.
    let shout = p.ask(&["callers", "Shout"]);
    has(&shout, "15 Main: var loud = \"hi\".Shout();");
    let callees = p.ask(&["callees", "Main"]);
    has(&callees, "ShapeExtensions.Doubled");
}

#[test]
fn events_and_delegates() {
    let p = shapes_project("events");
    let callers = p.ask(&["callers", "OnSent"]);
    has(&callers, "Bus.Sent in Main App/Program.cs:19");
    has(&callers, "Sent?.Invoke(message);");
    let refs = p.ask(&["refs", "Bus.Sent"]);
    has(&refs, "App/Program.cs");
}

#[test]
fn a_property_used_across_files() {
    let p = shapes_project("prop");
    let refs = p.ask(&["refs", "Shape.Id"]);
    has(&refs, "16 Main: c.Id = 3;");
    has(&refs, "17 Main: var i = c.Id;");
}

#[test]
fn using_static_and_same_namespace_types() {
    let p = shapes_project("static");
    has(&p.ask(&["callers", "Twice"]), "21 Main: Twice(2);");
    // `Helper` is in the namespace of `Program`, in another file, and needs no `using`.
    has(&p.ask(&["callers", "Helper.Run"]), "22 Main: Helper.Run();");
}

#[test]
fn test_methods_are_test_code() {
    let p = shapes_project("tests");
    // Without --tests the call from the test is named apart, without its line.
    let plain = p.ask(&["callers", "Circle.Area"]);
    has(&plain, "1 more in tests");
    has(&plain, "Acme.Tests/CircleTests.cs: AreaWorks:11");
    lacks(&plain, "new Circle().Area();");
    let with_tests = p.ask(&["callers", "Circle.Area", "--tests"]);
    has(&with_tests, "Acme.Tests/CircleTests.cs");
    has(&with_tests, "AreaWorks: new Circle().Area();");
}

#[test]
fn namespaces_aliases_and_global_usings() {
    let p = Project::new(
        "namespaces",
        &[
            ("GlobalUsings.cs", "global using Acme.Model;\n"),
            (
                "Model/Item.cs",
                "namespace Acme.Model\n{\n    public class Item\n    {\n        public static Item Create() => new Item();\n        public void Save() { }\n    }\n}\n",
            ),
            ("Model/Other/Item.cs", "namespace Acme.Other\n{\n    public class Item\n    {\n        public void Save() { }\n    }\n}\n"),
            ("Model/Linq/Token.cs", "namespace Acme.Model.Linq\n{\n    public class Token\n    {\n        public void Walk() { }\n    }\n}\n"),
            (
                "App/Use.cs",
                "using Json = Acme.Model.Item;\n\nnamespace Acme.Model.Views\n{\n    class Use\n    {\n        void Go()\n        {\n            var a = Item.Create();\n            a.Save();\n            var b = Json.Create();\n            var t = new Linq.Token();\n            t.Walk();\n        }\n    }\n}\n",
            ),
            ("App/Far.cs", "namespace Far.Away\n{\n    class Far\n    {\n        void Go(Item x) { x.Save(); }\n    }\n}\n"),
        ],
    );
    // `Item` comes in through the global using in another file, or from the namespace around.
    let callers = p.ask(&["callers", "Model/Item.cs:6"]);
    has(&callers, "10 Go: a.Save();");
    has(&callers, "5 Go: void Go(Item x) { x.Save(); }");
    // The type of the same name in another namespace is not what these files see.
    let other = p.ask(&["callers", "Model/Other/Item.cs:5"]);
    has(&other, "Model/Other/Item.cs:5");
    lacks(&other, "Go:");
    // `using Json = Acme.Model.Item;`
    has(&p.ask(&["callers", "Create"]), "11 Go: var b = Json.Create();");
    // `Linq.Token` inside `namespace Acme.Model.Views` is `Acme.Model.Linq.Token`.
    has(&p.ask(&["callers", "Token.Walk"]), "13 Go: t.Walk();");
}

#[test]
fn partial_types_and_conditional_code() {
    let p = Project::new(
        "partial",
        &[
            (
                "Reader.cs",
                "namespace Acme\n{\n    public partial class Reader\n    {\n        int Mode(int kind)\n        {\n            if (kind == 0)\n            {\n                return 0;\n            }\n#if EXTRA\n            else if (kind == 1)\n            {\n                return 1;\n            }\n#endif\n            else\n            {\n                Fill();\n            }\n            return 2;\n        }\n\n        public void Skip() { }\n    }\n}\n",
            ),
            ("Reader.Async.cs", "namespace Acme\n{\n    public partial class Reader\n    {\n        private void Fill() { Skip(); }\n    }\n}\n"),
        ],
    );
    // The `#if` that splits the `if` chain does not hide the methods after it.
    has(&p.ask(&["find", "Skip"]), "Reader.cs:24");
    // A private method of a partial class is called from its other part.
    has(&p.ask(&["callers", "Fill"]), "19 Mode: Fill();");
    has(&p.ask(&["callers", "Skip"]), "5 Fill: private void Fill() { Skip(); }");
}
